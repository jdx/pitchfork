mod common;

use common::TestEnv;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn test_daemon_custom_working_directory() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a subdirectory within the project
    let project_dir = env.project_dir();
    let custom_dir = project_dir.join("custom_work_dir");
    fs::create_dir_all(&custom_dir).unwrap();

    // Create a daemon that writes to its working directory
    let toml_content = r#"
[daemons.dir_test]
run = "bash -c 'pwd > working_dir.txt && sleep 5'"
dir = "custom_work_dir"
"#;
    env.create_toml(toml_content);

    // Start the daemon
    let output = env.run_command(&["start", "dir_test"]);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "Start command should succeed");

    // Wait for the daemon to write the file
    std::thread::sleep(Duration::from_millis(500));

    // Check that the file was written in the custom directory
    let output_file = custom_dir.join("working_dir.txt");
    assert!(
        output_file.exists(),
        "Daemon should have written to its custom working directory"
    );

    // Verify the working directory path in the file
    let written_dir = fs::read_to_string(&output_file).unwrap().trim().to_string();
    let expected_dir = custom_dir.canonicalize().unwrap();
    let written_path = PathBuf::from(&written_dir);

    assert_eq!(
        written_path.canonicalize().unwrap(),
        expected_dir,
        "Daemon should have run in the configured directory"
    );

    // Clean up
    let _ = env.run_command(&["stop", "dir_test"]);
}

#[test]
#[serial_test::serial]
fn test_daemon_working_directory_with_env_var() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a subdirectory within the project
    let project_dir = env.project_dir();
    let custom_dir = project_dir.join("env_work_dir");
    fs::create_dir_all(&custom_dir).unwrap();

    // Set an environment variable
    unsafe {
        std::env::set_var("PITCHFORK_TEST_WORKDIR", custom_dir.to_str().unwrap());
    }

    // Create a daemon that uses an environment variable in dir
    let toml_content = r#"
[daemons.env_dir_test]
run = "bash -c 'pwd > working_dir.txt && sleep 5'"
dir = "$PITCHFORK_TEST_WORKDIR"
"#;
    env.create_toml(toml_content);

    // Start the daemon with the environment variable
    let output = env.run_command_with_env(
        &["start", "env_dir_test"],
        &[("PITCHFORK_TEST_WORKDIR", custom_dir.to_str().unwrap())],
    );
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "Start command should succeed with env var expansion"
    );

    // Wait for the daemon to write the file
    std::thread::sleep(Duration::from_millis(500));

    // Check that the file was written in the custom directory
    let output_file = custom_dir.join("working_dir.txt");
    assert!(
        output_file.exists(),
        "Daemon should have written to the env-var-expanded working directory"
    );

    // Clean up
    unsafe {
        std::env::remove_var("PITCHFORK_TEST_WORKDIR");
    }
    let _ = env.run_command(&["stop", "env_dir_test"]);
}

#[test]
fn test_daemon_absolute_working_directory() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create an absolute path directory
    let project_dir = env.project_dir();
    let abs_dir = project_dir.join("absolute_work_dir");
    fs::create_dir_all(&abs_dir).unwrap();
    let abs_path = abs_dir.canonicalize().unwrap();

    // Create a daemon with an absolute path
    let toml_content = format!(
        r#"
[daemons.abs_dir_test]
run = "bash -c 'pwd > working_dir.txt && sleep 5'"
dir = "{}"
"#,
        abs_path.display()
    );
    env.create_toml(&toml_content);

    // Start the daemon
    let output = env.run_command(&["start", "abs_dir_test"]);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "Start command should succeed with absolute path"
    );

    // Wait for the daemon to write the file
    std::thread::sleep(Duration::from_millis(500));

    // Check that the file was written in the absolute directory
    let output_file = abs_path.join("working_dir.txt");
    assert!(
        output_file.exists(),
        "Daemon should have written to the absolute working directory"
    );

    // Clean up
    let _ = env.run_command(&["stop", "abs_dir_test"]);
}
