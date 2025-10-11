use pitchfork_cli::*;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

/// Get the path to a test script file
pub fn get_script_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("scripts")
        .join(file)
}

/// Helper struct for E2E test environment
pub struct TestEnv {
    temp_dir: TempDir,
    pitchfork_bin: PathBuf,
    home_dir: PathBuf,
}

impl TestEnv {
    /// Create a new test environment with isolated directories
    pub fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let home_dir = temp_dir.path().join("home");
        fs::create_dir_all(&home_dir).unwrap();

        // Get the pitchfork binary path
        let pitchfork_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("pitchfork");

        // Create state and logs directories
        let state_dir = home_dir.join(".local").join("state").join("pitchfork");
        fs::create_dir_all(&state_dir).unwrap();

        Self {
            temp_dir,
            pitchfork_bin,
            home_dir,
        }
    }

    /// Get the project directory path
    pub fn project_dir(&self) -> PathBuf {
        self.temp_dir.path().join("project")
    }

    /// Create a pitchfork.toml file with the given content
    pub fn create_toml(&self, content: &str) -> PathBuf {
        let project_dir = self.project_dir();
        fs::create_dir_all(&project_dir).unwrap();
        let toml_path = project_dir.join("pitchfork.toml");
        fs::write(&toml_path, content).unwrap();
        toml_path
    }

    /// Run a pitchfork command and return the output
    pub fn run_command(&self, args: &[&str]) -> std::process::Output {
        self.run_command_with_env(args, &[])
    }

    /// Run a pitchfork command with additional environment variables
    pub fn run_command_with_env(
        &self,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> std::process::Output {
        let mut cmd = Command::new(&self.pitchfork_bin);
        cmd.args(args)
            .current_dir(self.project_dir())
            .env("HOME", &self.home_dir)
            .env("PITCHFORK_LOG", "debug")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, val) in extra_env {
            cmd.env(key, val);
        }

        cmd.output().expect("Failed to execute pitchfork command")
    }

    /// Run a pitchfork command in the background
    #[allow(dead_code)]
    pub fn run_background(&self, args: &[&str]) -> std::process::Child {
        Command::new(&self.pitchfork_bin)
            .args(args)
            .current_dir(self.project_dir())
            .env("HOME", &self.home_dir)
            .env("PITCHFORK_LOG", "debug")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn pitchfork command")
    }

    /// Wait for a specific duration
    pub fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    /// Check if the pitchfork binary exists, build if necessary
    pub fn ensure_binary_exists(&self) -> Result<()> {
        if !self.pitchfork_bin.exists() {
            eprintln!("Building pitchfork binary...");
            let status = Command::new("cargo")
                .args(["build"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .status()
                .expect("Failed to build pitchfork");

            if !status.success() {
                panic!("Failed to build pitchfork binary");
            }
        }
        Ok(())
    }

    /// Read log file for a daemon
    pub fn read_logs(&self, daemon_id: &str) -> String {
        let log_path = self
            .home_dir
            .join(".local")
            .join("state")
            .join("pitchfork")
            .join("logs")
            .join(daemon_id)
            .join(format!("{}.log", daemon_id));

        fs::read_to_string(log_path).unwrap_or_default()
    }

    /// Get the state file path
    #[allow(dead_code)]
    pub fn state_file_path(&self) -> PathBuf {
        self.home_dir
            .join(".local")
            .join("state")
            .join("pitchfork")
            .join("state.toml")
    }

    /// Get daemon status by running `pitchfork status <id>`
    #[allow(dead_code)]
    pub fn get_daemon_status(&self, daemon_id: &str) -> Option<String> {
        let output = self.run_command(&["status", daemon_id]);

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse output like:
        // Name: docs
        // PID: 51580
        // Status: running
        for line in stdout.lines() {
            if line.trim().starts_with("Status:") {
                let status = line.split(':').nth(1)?.trim().to_string();
                return Some(status);
            }
        }
        None
    }

    /// Cleanup all processes and supervisor
    pub fn cleanup(&self) {
        let _ = self.run_command(&["supervisor", "stop"]);
        std::thread::sleep(Duration::from_millis(500));
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        self.cleanup();
    }
}
