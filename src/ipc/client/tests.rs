use super::*;

#[cfg(unix)] // The mock server uses a Unix socket; Windows uses named pipes.
async fn client_pair() -> Result<(IpcClient, tokio::net::UnixStream, tempfile::TempDir)> {
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    use miette::IntoDiagnostic;

    let dir = tempfile::tempdir().into_diagnostic()?;
    let path = dir.path().join("client.sock");
    let listener = tokio::net::UnixListener::bind(&path).into_diagnostic()?;
    let stream = IpcStream::connect(path.to_fs_name::<GenericFilePath>().into_diagnostic()?)
        .await
        .into_diagnostic()?;
    let (server, _) = listener.accept().await.into_diagnostic()?;
    let (recv, send) = tokio::io::split(stream);
    Ok((
        IpcClient {
            _id: "test".to_string(),
            recv: Mutex::new(BufReader::new(recv)),
            send: Mutex::new(send),
            exchange: Mutex::new(()),
            desynced: std::sync::atomic::AtomicBool::new(false),
        },
        server,
        dir,
    ))
}

// https://github.com/jdx/pitchfork/pull/1108#discussion_r4502497242
/// The server models an existing start with a 120s delay; the joining
/// request asks for 0s. Paused time crosses its former 60s client deadline.
#[cfg(unix)] // The mock server uses a Unix socket; Windows uses named pipes.
#[tokio::test(start_paused = true)]
async fn a_joined_start_waits_for_the_original_readiness_deadline() -> Result<()> {
    use miette::IntoDiagnostic;

    let (client, server, _dir) = client_pair().await?;
    let id = DaemonId::new("proj", "slow");
    let opts = RunOptions {
        id: id.clone(),
        wait_ready: true,
        ready_delay: Some(0),
        ..Default::default()
    };
    let respond = async {
        let mut server = BufReader::new(server);
        let mut bytes = Vec::new();
        server.read_until(0, &mut bytes).await.into_diagnostic()?;
        assert!(matches!(
            deserialize::<IpcRequest>(&bytes)?,
            IpcRequest::Run(RunOptions {
                ready_delay: Some(0),
                ..
            })
        ));
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut response = serialize(&IpcResponse::DaemonReady {
            daemon: Daemon {
                id,
                ready_delay: Some(120),
                ..Default::default()
            },
        })?;
        response.push(0);
        server
            .get_mut()
            .write_all(&response)
            .await
            .into_diagnostic()
    };
    let (result, sent) = tokio::join!(client.run(opts), respond);
    sent?;
    assert!(result?.started);
    Ok(())
}

#[test]
fn records_are_those_of_the_asked_ids_that_state_has() {
    let mut state = crate::state_file::StateFile::new(PathBuf::from("state.toml"));
    let known = DaemonId::new("proj", "known");
    state.daemons.insert(
        known.clone(),
        Daemon {
            id: known.clone(),
            ..Default::default()
        },
    );
    let records = records_of(&state, &[known.clone(), DaemonId::new("proj", "unknown")]);
    assert_eq!(
        records.into_iter().map(|d| d.id).collect::<Vec<_>>(),
        vec![known]
    );
}

#[test]
fn an_older_supervisor_reports_get_daemons_as_an_unknown_request() {
    // What the supervisor answers a request it cannot deserialize.
    assert!(is_unknown_request(
        "Invalid request: unknown variant `GetDaemons`"
    ));
}

#[test]
fn stop_request_timeout_adds_verification_and_slack() {
    assert_eq!(
        stop_request_timeout(Duration::from_secs(5), Duration::from_secs(10)),
        Duration::from_secs(17)
    );
}

#[test]
fn stop_request_timeout_saturates_for_a_huge_stop_budget() {
    assert_eq!(
        stop_request_timeout(Duration::MAX, Duration::from_secs(10)),
        Duration::MAX
    );
}
