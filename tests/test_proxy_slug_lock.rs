//! Regression test for the `proxy add`/`proxy remove` fslock self-deadlock.
//!
//! `add_slug_with_namespace()` and `remove_slug()` hold the global config
//! fslock across a read-modify-write. They used to call
//! `sync_hosts_from_settings()` while still holding it; that function re-reads
//! the global config via `PitchforkToml::read()`, which acquires the same
//! lock. flock(2) locks are per open file description, so the second
//! acquisition in the same process blocked forever against the first.
//!
//! This test lives in its own integration-test binary on purpose: the config
//! path and settings are process-wide lazies initialized from env vars, so it
//! needs a process where no other test has touched them first.

use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(30);

/// Wait for a worker thread to finish, failing after `DEADLINE`.
///
/// The workers run on plain detached threads, deliberately NOT on a tokio
/// runtime via `spawn_blocking`: tokio joins already-started blocking tasks
/// during runtime teardown, so a worker deadlocked in `flock` would hang the
/// entire test run there instead of letting the deadline assertion report a
/// clean failure (verified against the pre-fix code).
fn join_within<T>(handle: std::thread::JoinHandle<T>, what: &str) -> pitchfork_cli::Result<T> {
    let start = Instant::now();
    while !handle.is_finished() {
        miette::ensure!(
            start.elapsed() < DEADLINE,
            "proxy slug {what} deadlocked: the global config fslock was \
             re-acquired while already held (fslock re-entrancy regression)"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    handle
        .join()
        .map_err(|_| miette::miette!("proxy slug {what} worker panicked"))
}

#[test]
fn test_proxy_slug_add_remove_does_not_self_deadlock() -> pitchfork_cli::Result<()> {
    use miette::IntoDiagnostic;
    use pitchfork_cli::pitchfork_toml::PitchforkToml;

    let temp_dir = tempfile::TempDir::new().into_diagnostic()?;

    // Keep the hosts sync away from the real system hosts file.
    let hosts_file = temp_dir.path().join("hosts");
    std::fs::write(&hosts_file, "127.0.0.1 localhost\n").into_diagnostic()?;

    // SAFETY: this is the only test in this binary, so no other thread is
    // reading or writing the environment concurrently.
    unsafe {
        std::env::set_var("PITCHFORK_CONFIG_DIR", temp_dir.path());
        std::env::set_var("PITCHFORK_HOSTS_FILE", &hosts_file);
        // The deadlock only triggered when the hosts sync actually ran
        // (proxy.enable && proxy.sync_hosts); sync_hosts defaults to true.
        std::env::set_var("PITCHFORK_PROXY_ENABLE", "true");
    }

    // The config path lazy must have picked up our override; anything else
    // means the test's process isolation is broken and running on would write
    // to the user's real config.
    let config_path = pitchfork_cli::env::PITCHFORK_GLOBAL_CONFIG_USER.clone();
    miette::ensure!(
        config_path.parent() == Some(temp_dir.path()),
        "PITCHFORK_GLOBAL_CONFIG_USER was initialized before this test set \
         PITCHFORK_CONFIG_DIR; this test must be the only one in its binary"
    );

    let add = std::thread::spawn(|| {
        PitchforkToml::add_slug_with_namespace("pf-lock-test", Some("pf-lock-ns"), Some("server"))
    });
    join_within(add, "add")??;

    // The hosts sync must actually have run — it is the code path that used to
    // re-acquire the lock. If it were skipped, this test would prove nothing.
    let hosts = std::fs::read_to_string(&hosts_file).into_diagnostic()?;
    miette::ensure!(
        hosts.contains("pf-lock-test"),
        "hosts sync did not run; the deadlock code path was not exercised"
    );

    let remove = std::thread::spawn(|| PitchforkToml::remove_slug("pf-lock-test"));
    let removed = join_within(remove, "remove")??;
    miette::ensure!(removed, "slug should have been added and then removed");

    // Both the config and the synced hosts file must be clean again.
    let raw = std::fs::read_to_string(&config_path).into_diagnostic()?;
    miette::ensure!(
        !raw.contains("pf-lock-test"),
        "slug must be gone from the written config"
    );
    let hosts = std::fs::read_to_string(&hosts_file).into_diagnostic()?;
    miette::ensure!(
        !hosts.contains("pf-lock-test"),
        "slug must be gone from the synced hosts file"
    );

    Ok(())
}
