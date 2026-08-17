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

#[test]
fn test_proxy_slug_add_remove_does_not_self_deadlock() {
    let temp_dir = tempfile::TempDir::new().unwrap();

    // SAFETY: this is the only test in this binary, so no other thread is
    // reading or writing the environment concurrently.
    unsafe {
        std::env::set_var("PITCHFORK_CONFIG_DIR", temp_dir.path());
        // The deadlock only triggers when the hosts sync actually runs
        // (proxy.enable && proxy.sync_hosts); sync_hosts defaults to true.
        std::env::set_var("PITCHFORK_PROXY_ENABLE", "true");
    }

    // If another test in this process initialized the config path lazy before
    // us, bail out rather than write to the user's real config.
    let config_path = pitchfork_cli::env::PITCHFORK_GLOBAL_CONFIG_USER.clone();
    if config_path.parent() != Some(temp_dir.path()) {
        eprintln!("skipping: PITCHFORK_GLOBAL_CONFIG_USER already initialized elsewhere");
        return;
    }

    // Run the add + remove on a separate thread so a regression shows up as a
    // clean test failure after the deadline instead of a hung test binary.
    let worker = std::thread::spawn(|| -> pitchfork_cli::Result<bool> {
        use pitchfork_cli::pitchfork_toml::PitchforkToml;
        PitchforkToml::add_slug_with_namespace("pf-lock-test", Some("pf-lock-ns"), Some("server"))?;
        PitchforkToml::remove_slug("pf-lock-test")
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    while !worker.is_finished() {
        assert!(
            Instant::now() < deadline,
            "proxy slug add/remove deadlocked: the global config fslock was \
             re-acquired while already held (fslock re-entrancy regression)"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let removed = worker.join().unwrap().unwrap();
    assert!(removed, "slug should have been added and then removed");

    // The slug must be gone from the written config again.
    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(!raw.contains("pf-lock-test"));
}
