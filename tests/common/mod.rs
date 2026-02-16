#![allow(dead_code)]

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
            .join(format!("{daemon_id}.log"));

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

    /// Run a pitchfork command from a specific directory
    #[allow(dead_code)]
    pub fn run_command_in_dir(&self, args: &[&str], dir: &PathBuf) -> std::process::Output {
        self.run_command_in_dir_with_env(args, dir, &[])
    }

    /// Run a pitchfork command from a specific directory with extra env vars
    #[allow(dead_code)]
    pub fn run_command_in_dir_with_env(
        &self,
        args: &[&str],
        dir: &PathBuf,
        extra_env: &[(&str, &str)],
    ) -> std::process::Output {
        let mut cmd = Command::new(&self.pitchfork_bin);
        cmd.args(args)
            .current_dir(dir)
            .env("HOME", &self.home_dir)
            .env("PITCHFORK_LOG", "debug")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, val) in extra_env {
            cmd.env(key, val);
        }

        cmd.output().expect("Failed to execute pitchfork command")
    }

    /// Create an alternate directory (for testing directory changes)
    #[allow(dead_code)]
    pub fn create_other_dir(&self) -> PathBuf {
        let other_dir = self.temp_dir.path().join("other");
        fs::create_dir_all(&other_dir).unwrap();
        other_dir
    }

    /// Get a path for a marker file in the temp directory
    pub fn marker_path(&self, name: &str) -> PathBuf {
        self.temp_dir.path().join(format!("{name}_marker"))
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

    /// Read daemon status directly from the state file on disk (bypasses CLI/IPC)
    #[allow(dead_code)]
    pub fn get_daemon_status_from_state_file(&self, daemon_id: &str) -> Option<String> {
        let state_path = self.state_file_path();
        let raw = fs::read_to_string(&state_path).ok()?;
        let state: toml::Value = toml::from_str(&raw).ok()?;
        state
            .get("daemons")?
            .get(daemon_id)?
            .get("status")?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Poll daemon status until it matches the expected value.
    /// Retries up to 50 times with 200ms intervals (10s total) to handle state file
    /// write delays that can occur under CI load.
    #[allow(dead_code)]
    pub fn wait_for_status(&self, daemon_id: &str, expected: &str) {
        for i in 0..50 {
            // Try CLI status first
            let status = self.get_daemon_status(daemon_id);
            if status.as_deref() == Some(expected) {
                return;
            }
            // Also try reading state file directly as fallback
            let file_status = self.get_daemon_status_from_state_file(daemon_id);
            if file_status.as_deref() == Some(expected) {
                return;
            }
            if i < 49 {
                std::thread::sleep(Duration::from_millis(200));
            } else {
                // Gather diagnostics for debugging CI failures
                let state_path = self.state_file_path();
                let state_contents = fs::read_to_string(&state_path)
                    .unwrap_or_else(|e| format!("<read error: {e}>"));
                let status_output = self.run_command(&["status", daemon_id]);
                let status_stderr = String::from_utf8_lossy(&status_output.stderr);
                let status_stdout = String::from_utf8_lossy(&status_output.stdout);
                panic!(
                    "Daemon {daemon_id} did not reach status '{expected}' after 10s\n\
                     CLI status: {status:?}\n\
                     State file status: {file_status:?}\n\
                     Status exit code: {}\n\
                     Status stdout: {status_stdout}\n\
                     Status stderr: {status_stderr}\n\
                     State file ({}):\n{state_contents}",
                    status_output.status,
                    state_path.display(),
                );
            }
        }
    }

    /// Get daemon PID by running `pitchfork status <id>`
    #[allow(dead_code)]
    pub fn get_daemon_pid(&self, daemon_id: &str) -> Option<u32> {
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
            if line.trim().starts_with("PID:") {
                let pid_str = line.split(':').nth(1)?.trim();
                if pid_str == "-" || pid_str.is_empty() {
                    return None;
                }
                return pid_str.parse().ok();
            }
        }
        None
    }

    /// Kill any process listening on the specified port
    #[cfg(unix)]
    pub fn kill_port(&self, port: u16) {
        use std::process::Command;
        // Use lsof to find process using the port, then kill it
        let output = Command::new("lsof")
            .args(["-ti", &format!(":{port}")])
            .output();

        if let Ok(output) = output
            && output.status.success()
        {
            let pids = String::from_utf8_lossy(&output.stdout);
            for pid in pids.lines() {
                if let Ok(pid) = pid.trim().parse::<i32>() {
                    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
                }
            }
            // Give the OS time to release the port
            std::thread::sleep(Duration::from_millis(100));
        }
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
