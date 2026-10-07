//! `register_namespace` must not retarget a namespace whose old directory is
//! gone while it still has a running daemon: the new project would see the old
//! project's daemons. Own binary: config/state paths are process-wide lazies.

#[test]
fn register_namespace_refuses_to_retarget_a_namespace_with_running_daemons() {
    use pitchfork_cli::pitchfork_toml::PitchforkToml;

    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("cfg");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let gone = tmp.path().join("gone");
    let new = tmp.path().join("new");
    std::fs::create_dir_all(&new).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            "[namespaces.shop]\ndir = {:?}\n",
            gone.display().to_string()
        ),
    )
    .unwrap();
    std::fs::write(
        state.join("state.toml"),
        "disabled = []\n\n[daemons.\"shop/web\"]\nid = \"shop/web\"\npid = 4242\n\n[daemons.\"shop/web\".status]\nrunning = {}\n",
    )
    .unwrap();
    // SAFETY: the only test in this binary.
    unsafe {
        std::env::set_var("PITCHFORK_CONFIG_DIR", &cfg);
        std::env::set_var("PITCHFORK_STATE_DIR", &state);
    }
    assert_eq!(
        pitchfork_cli::env::PITCHFORK_GLOBAL_CONFIG_USER.parent(),
        Some(cfg.as_path()),
        "env was initialized before the test set it"
    );

    let err = PitchforkToml::register_namespace("shop", &new.to_string_lossy())
        .expect_err("running daemons must block retargeting");
    assert!(err.to_string().contains("running daemons"), "{err}");

    // Once the daemon is gone from state, the same call succeeds.
    std::fs::write(state.join("state.toml"), "disabled = []\n").unwrap();
    PitchforkToml::register_namespace("shop", &new.to_string_lossy()).unwrap();
    let raw = std::fs::read_to_string(cfg.join("config.toml")).unwrap();
    assert!(raw.contains("new"), "{raw}");
}
