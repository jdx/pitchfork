#![warn(rust_2018_idioms)]
#![cfg(unix)] // The daemon uses a Unix shell and process-group termination.

use super::*;
use crate::pitchfork_toml::{Dir, Retry};

// https://github.com/jdx/pitchfork/pull/1108#discussion_r4505580959
/// A subprocess isolates the supervisor's cached configuration and state.
/// Canceling the initiating future must leave its monitor's result available.
#[test]
fn canceling_a_start_keeps_its_pending_readiness_joinable() -> Result<()> {
    const CHILD: &str = "PITCHFORK_TEST_CANCEL_START";
    if std::env::var_os(CHILD).is_none() {
        let temp = tempfile::tempdir().into_diagnostic()?;
        let output = std::process::Command::new(std::env::current_exe().into_diagnostic()?)
            .args([
                "--exact",
                "supervisor::lifecycle::readiness_tests::canceling_a_start_keeps_its_pending_readiness_joinable",
                "--nocapture",
            ])
            .current_dir(temp.path())
            .env(CHILD, "1")
            .env("PITCHFORK_STATE_DIR", temp.path().join("state"))
            .env("PITCHFORK_CONFIG_DIR", temp.path().join("config"))
            .env("PITCHFORK_LOGS_DIR", temp.path().join("logs"))
            .env("PITCHFORK_SHELL", "sh -c")
            .output()
            .into_diagnostic()?;
        assert!(
            output.status.success(),
            "isolated readiness test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        return Ok(());
    }

    tokio::runtime::Runtime::new()
        .into_diagnostic()?
        .block_on(async {
            for retry in [0, 1] {
                let id = DaemonId::new("cancel-start", format!("retry-{retry}"));
                let release = format!("release-{retry}");
                let opts = RunOptions {
                    id: id.clone(),
                    run: Some(format!(
                        "while ! test -e {release}; do sleep 0.1; done; echo READY; sleep 30"
                    )),
                    cmd: vec!["sh".into()],
                    dir: Dir(std::env::current_dir().into_diagnostic()?),
                    wait_ready: true,
                    retry: Retry(retry),
                    ready_output: Some(ReadyOutput {
                        pattern: "READY".into(),
                        timeout: Some(Duration::from_secs(10)),
                    }),
                    mise: Some(false),
                    // A PTY keeps output in this process; no IPC sink is needed.
                    pty: Some(true),
                    ..RunOptions::default()
                };
                let first_opts = opts.clone();
                let first = tokio::spawn(async move { SUPERVISOR.run(first_opts).await });
                let spawned = tokio::time::timeout(Duration::from_secs(10), async {
                    while SUPERVISOR.ready_check(&id).is_none() {
                        tokio::task::yield_now().await;
                    }
                })
                .await;
                first.abort();
                let _ = first.await;
                let mut joined = tokio::spawn(async move { SUPERVISOR.run(opts).await });
                // The daemon is not ready until the release file exists.
                let early = tokio::time::timeout(Duration::from_millis(500), &mut joined).await;
                let returned_early = early.is_ok();
                std::fs::write(release, "").into_diagnostic()?;
                let result = match early {
                    Ok(result) => Ok(result),
                    Err(_) => tokio::time::timeout(Duration::from_secs(10), joined).await,
                };
                SUPERVISOR.stop(&id).await?;
                assert!(
                    spawned.is_ok(),
                    "the initiating start did not register readiness: {spawned:?}"
                );
                assert!(!returned_early, "joining start returned early: {result:?}");
                assert!(
                    matches!(result, Ok(Ok(Ok(IpcResponse::DaemonReady { .. })))),
                    "joining start returned {result:?}"
                );
            }
            Ok(())
        })
}
