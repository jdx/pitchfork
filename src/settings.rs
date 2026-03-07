//! User-configurable settings for pitchfork.
//!
//! Settings can be configured in multiple ways (in order of precedence):
//! 1. Environment variables (highest priority)
//! 2. Project-level `pitchfork.toml` or `pitchfork.local.toml` (in `[settings]` section)
//! 3. User-level `~/.config/pitchfork/config.toml` (in `[settings]` section)
//! 4. System-level `/etc/pitchfork/config.toml` (in `[settings]` section)
//! 5. Built-in defaults (lowest priority)
//!
//! Example pitchfork.toml with settings:
//! ```toml
//! [daemons.myapp]
//! run = "node server.js"
//!
//! [settings.general]
//! autostop_delay = "5m"
//! log_level = "debug"
//!
//! [settings.web]
//! auto_start = true
//! ```
//!
//! This module is generated from `settings.toml` at build time.

// Include the generated code from build.rs.
// Wrapped in a module so that `#[allow(clippy::all)]` suppresses all clippy
// warnings for the generated code without affecting the rest of this file.
#[allow(clippy::all)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated/settings.rs"));
}
pub use generated::*;

// Include metadata for introspection
#[allow(clippy::all, dead_code)]
mod meta {
    include!(concat!(env!("OUT_DIR"), "/generated/settings_meta.rs"));
}

#[allow(unused_imports)]
pub use meta::*;

impl Settings {
    /// Resolve the mise binary path.
    ///
    /// If `general.mise_bin` is explicitly set, returns that path.
    /// Otherwise, searches well-known install locations:
    /// - `~/.local/bin/mise`
    /// - `~/.cargo/bin/mise`
    /// - `/usr/local/bin/mise`
    /// - `/opt/homebrew/bin/mise`
    ///
    /// Returns `None` if mise cannot be found.
    pub fn resolve_mise_bin(&self) -> Option<std::path::PathBuf> {
        use std::path::PathBuf;

        // Explicit configuration takes priority
        if !self.general.mise_bin.is_empty() {
            let p = PathBuf::from(&self.general.mise_bin);
            if p.is_file() {
                return Some(p);
            }
            warn!(
                "mise_bin is set to {:?} but the file does not exist",
                self.general.mise_bin
            );
            return None;
        }

        // Search well-known install paths
        let home = crate::env::HOME_DIR.as_path();
        let candidates = [
            home.join(".local/bin/mise"),
            home.join(".cargo/bin/mise"),
            PathBuf::from("/usr/local/bin/mise"),
            PathBuf::from("/opt/homebrew/bin/mise"),
        ];

        candidates.into_iter().find(|p| p.is_file())
    }

    /// Return `supervisor.port_bump_attempts` as `u32`, clamping out-of-range
    /// values to the schema default (10).
    ///
    /// This is the single source of truth for the fallback so that call-sites
    /// don't each duplicate the hardcoded `10`.
    pub fn default_port_bump_attempts(&self) -> u32 {
        u32::try_from(self.supervisor.port_bump_attempts).unwrap_or(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();

        // Test general settings
        assert_eq!(settings.general.autostop_delay, "1m");
        assert_eq!(settings.general.interval, "10s");
        assert_eq!(settings.general.log_level, "info");

        // Test IPC settings
        assert_eq!(settings.ipc.connect_attempts, 5);
        assert_eq!(settings.ipc.request_timeout, "5s");
        assert_eq!(settings.ipc.rate_limit, 100);

        // Test web settings
        assert!(!settings.web.auto_start);
        assert_eq!(settings.web.bind_address, "127.0.0.1");
        assert_eq!(settings.web.bind_port, 3120);
        assert_eq!(settings.web.log_lines, 100);

        // Test TUI settings
        assert_eq!(settings.tui.refresh_rate, "2s");
        assert_eq!(settings.tui.stat_history, 60);

        // Test supervisor settings
        assert_eq!(settings.supervisor.ready_check_interval, "500ms");
        assert_eq!(settings.supervisor.file_watch_debounce, "1s");
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(Settings::parse_duration("1s"), Some(Duration::from_secs(1)));
        assert_eq!(
            Settings::parse_duration("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            Settings::parse_duration("1m"),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            Settings::parse_duration("2h"),
            Some(Duration::from_secs(7200))
        );
        assert_eq!(Settings::parse_duration("invalid"), None);
    }

    #[test]
    fn test_convenience_methods() {
        let settings = Settings::default();

        assert_eq!(settings.general_autostop_delay(), Duration::from_secs(60));
        assert_eq!(settings.general_interval(), Duration::from_secs(10));
    }

    #[test]
    fn test_load_from_toml_string() {
        // Test loading from a complete TOML string
        let toml_content = r#"
[general]
autostop_delay = "5m"
interval = "30s"
log_level = "debug"

[ipc]
connect_attempts = 10
request_timeout = "10s"

[web]
auto_start = true
bind_port = 8080
"#;

        let settings: Settings = toml::from_str(toml_content).unwrap();

        // Explicitly set values
        assert_eq!(settings.general.autostop_delay, "5m");
        assert_eq!(settings.general.interval, "30s");
        assert_eq!(settings.general.log_level, "debug");
        assert_eq!(settings.ipc.connect_attempts, 10);
        assert_eq!(settings.ipc.request_timeout, "10s");
        assert!(settings.web.auto_start);
        assert_eq!(settings.web.bind_port, 8080);
        assert_eq!(settings.general.log_file_level, "info");
        assert_eq!(settings.ipc.rate_limit, 100);
        assert_eq!(settings.web.bind_address, "127.0.0.1");
        assert_eq!(settings.tui.refresh_rate, "2s");
    }

    #[test]
    fn test_partial_config_uses_defaults() {
        // Test that missing sections use defaults
        let toml_content = r#"
[general]
log_level = "warn"
"#;

        let settings: Settings = toml::from_str(toml_content).unwrap();

        // Explicitly set value
        assert_eq!(settings.general.log_level, "warn");

        // All other values should be defaults
        assert_eq!(settings.general.autostop_delay, "1m");
        assert_eq!(settings.general.interval, "10s");
        assert_eq!(settings.ipc.connect_attempts, 5);
        assert!(!settings.web.auto_start);
        assert_eq!(settings.tui.stat_history, 60);
        assert_eq!(settings.supervisor.stop_timeout, "5s");
    }

    #[test]
    fn test_empty_config_uses_all_defaults() {
        // Empty TOML should result in all defaults
        let settings: Settings = toml::from_str("").unwrap();

        assert_eq!(settings.general.autostop_delay, "1m");
        assert_eq!(settings.general.interval, "10s");
        assert_eq!(settings.general.log_level, "info");
        assert_eq!(settings.ipc.connect_attempts, 5);
        assert!(!settings.web.auto_start);
        assert_eq!(settings.tui.refresh_rate, "2s");
    }

    #[test]
    fn test_env_override() {
        // Test load_from_env() directly on a fresh Settings instance.
        // NOTE: We deliberately do NOT use settings() here. settings() is a
        // process-wide OnceLock singleton that is initialized exactly once, so
        // any env-var changes made after first access would be invisible to it.
        // By calling Settings::default() + load_from_env() directly we get a
        // proper unit test of the env-reading code path.
        //
        // Cargo runs tests in the same process on multiple threads. Mutating
        // env vars from concurrent threads is a data race (UB in Rust's memory
        // model). We therefore hold a process-wide mutex for the entire
        // set/test/unset sequence so that at most one test touches the env at
        // a time.
        use std::sync::{LazyLock, Mutex};
        static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        // SAFETY: we hold ENV_LOCK so no other thread in this process is
        // concurrently reading or writing these variables.
        unsafe {
            std::env::set_var("PITCHFORK_AUTOSTOP_DELAY", "10m");
            std::env::set_var("PITCHFORK_INTERVAL", "5s");
            std::env::set_var("PITCHFORK_IPC_CONNECT_ATTEMPTS", "20");
            std::env::set_var("PITCHFORK_WEB_AUTO_START", "true");
        }

        let mut settings = Settings::default();
        settings.load_from_env();

        // Verify the env vars were picked up
        assert_eq!(settings.general.autostop_delay, "10m");
        assert_eq!(settings.general.interval, "5s");
        assert_eq!(settings.ipc.connect_attempts, 20);
        assert!(settings.web.auto_start);

        // Fields with no corresponding env var set remain at defaults
        assert_eq!(settings.general.log_level, "info");
        assert_eq!(settings.ipc.rate_limit, 100);

        // Clean up to avoid polluting other tests.
        // SAFETY: same guarantee as above – we still hold ENV_LOCK.
        unsafe {
            std::env::remove_var("PITCHFORK_AUTOSTOP_DELAY");
            std::env::remove_var("PITCHFORK_INTERVAL");
            std::env::remove_var("PITCHFORK_IPC_CONNECT_ATTEMPTS");
            std::env::remove_var("PITCHFORK_WEB_AUTO_START");
        }
    }

    #[test]
    fn test_invalid_duration_fallback() {
        let mut settings = Settings::default();

        // Set invalid duration values
        settings.general.autostop_delay = "invalid".to_string();
        settings.general.interval = "not_a_duration".to_string();

        // Convenience methods should fallback to default values
        assert_eq!(settings.general_autostop_delay(), Duration::from_secs(60)); // default "1m"
        assert_eq!(settings.general_interval(), Duration::from_secs(10)); // default "10s"
    }

    #[test]
    fn test_duration_methods_all_fields() {
        let settings = Settings::default();

        // Test all Duration convenience methods return expected defaults
        assert_eq!(settings.general_autostop_delay(), Duration::from_secs(60));
        assert_eq!(settings.general_interval(), Duration::from_secs(10));
        assert_eq!(settings.ipc_connect_min_delay(), Duration::from_millis(100));
        assert_eq!(settings.ipc_connect_max_delay(), Duration::from_secs(1));
        assert_eq!(settings.ipc_request_timeout(), Duration::from_secs(5));
        assert_eq!(settings.ipc_rate_limit_window(), Duration::from_secs(1));
        assert_eq!(settings.web_sse_poll_interval(), Duration::from_millis(500));
        assert_eq!(settings.tui_refresh_rate(), Duration::from_secs(2));
        assert_eq!(settings.tui_tick_rate(), Duration::from_millis(100));
        assert_eq!(settings.tui_message_duration(), Duration::from_secs(3));
        assert_eq!(
            settings.supervisor_ready_check_interval(),
            Duration::from_millis(500)
        );
        assert_eq!(
            settings.supervisor_file_watch_debounce(),
            Duration::from_secs(1)
        );
        assert_eq!(
            settings.supervisor_log_flush_interval(),
            Duration::from_millis(500)
        );
        assert_eq!(settings.supervisor_stop_timeout(), Duration::from_secs(5));
        assert_eq!(
            settings.supervisor_restart_delay(),
            Duration::from_millis(100)
        );
        assert_eq!(
            settings.supervisor_cron_check_interval(),
            Duration::from_secs(10)
        );
        assert_eq!(
            settings.supervisor_http_client_timeout(),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn test_unknown_fields_ignored() {
        // serde's default behaviour (without #[serde(deny_unknown_fields)]) is to
        // silently discard unrecognised keys.  Our generated structs rely on this
        // so that future pitchfork versions with new settings don't break older
        // configs – and so that users can add comments/custom keys without errors.
        let toml_content = r#"
[general]
log_level = "debug"
unknown_field = "should be ignored"

[unknown_section]
foo = "bar"
"#;

        let result: Result<Settings, _> = toml::from_str(toml_content);
        // Must succeed: our structs do NOT use deny_unknown_fields.
        let settings = result.expect("unknown fields should be silently ignored by serde");
        assert_eq!(settings.general.log_level, "debug");
        // Known fields in unrecognised sections (unknown_section) are dropped;
        // all other fields fall back to their defaults.
        assert_eq!(settings.general.autostop_delay, "1m");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let settings = Settings::default();

        // Serialize to TOML
        let toml_str = toml::to_string_pretty(&settings).unwrap();

        // Parse back
        let parsed: Settings = toml::from_str(&toml_str).unwrap();

        // Verify roundtrip
        assert_eq!(
            settings.general.autostop_delay,
            parsed.general.autostop_delay
        );
        assert_eq!(settings.general.interval, parsed.general.interval);
        assert_eq!(settings.ipc.connect_attempts, parsed.ipc.connect_attempts);
        assert_eq!(settings.web.auto_start, parsed.web.auto_start);
        assert_eq!(settings.tui.stat_history, parsed.tui.stat_history);
    }

    #[test]
    fn test_type_coercion() {
        // Test that integer and boolean values are correctly parsed
        let toml_content = r#"
[ipc]
connect_attempts = 3
rate_limit = 50

[web]
auto_start = true
bind_port = 9000
log_lines = 200

[tui]
stat_history = 120
"#;

        let settings: Settings = toml::from_str(toml_content).unwrap();

        assert_eq!(settings.ipc.connect_attempts, 3);
        assert_eq!(settings.ipc.rate_limit, 50);
        assert!(settings.web.auto_start);
        assert_eq!(settings.web.bind_port, 9000);
        assert_eq!(settings.web.log_lines, 200);
        assert_eq!(settings.tui.stat_history, 120);
    }

    #[test]
    fn test_merge_from_non_default_values() {
        let mut base = Settings::default();

        // Build a partial with only the values we want to override
        let mut partial = SettingsPartial::default();
        partial.general.autostop_delay = Some("5m".to_string());
        partial.general.log_level = Some("debug".to_string());
        partial.ipc.connect_attempts = Some(10);
        partial.web.auto_start = Some(true);

        // Apply
        base.apply_partial(&partial);

        // Explicitly set values should be applied
        assert_eq!(base.general.autostop_delay, "5m");
        assert_eq!(base.general.log_level, "debug");
        assert_eq!(base.ipc.connect_attempts, 10);
        assert!(base.web.auto_start);

        // Unset fields in partial remain at base defaults
        assert_eq!(base.general.interval, "10s");
        assert_eq!(base.ipc.rate_limit, 100);
    }

    #[test]
    fn test_merge_from_preserves_existing() {
        let mut base = Settings::default();
        base.general.autostop_delay = "2m".to_string();
        base.web.bind_port = 8080;

        // An empty partial has all-None fields - nothing should change
        let empty_partial = SettingsPartial::default();
        base.apply_partial(&empty_partial);

        assert_eq!(base.general.autostop_delay, "2m"); // preserved
        assert_eq!(base.web.bind_port, 8080); // preserved
    }

    #[test]
    fn test_merge_chain() {
        // Simulate system -> user -> project merge chain
        let mut settings = Settings::default();

        // System config: set some values
        let mut system_partial = SettingsPartial::default();
        system_partial.general.log_level = Some("warn".to_string());
        system_partial.web.bind_address = Some("0.0.0.0".to_string());
        settings.apply_partial(&system_partial);

        // User config: override log_level back to info, add tui setting
        let mut user_partial = SettingsPartial::default();
        user_partial.general.log_level = Some("info".to_string());
        user_partial.tui.refresh_rate = Some("1s".to_string());
        settings.apply_partial(&user_partial);

        // Project config: override log_level to debug, enable web
        let mut project_partial = SettingsPartial::default();
        project_partial.general.log_level = Some("debug".to_string());
        project_partial.web.auto_start = Some(true);
        settings.apply_partial(&project_partial);

        // Verify final merged state
        assert_eq!(settings.general.log_level, "debug"); // from project
        assert_eq!(settings.web.bind_address, "0.0.0.0"); // from system (not overridden)
        assert_eq!(settings.tui.refresh_rate, "1s"); // from user
        assert!(settings.web.auto_start); // from project

        // Also verify Bug 5 fix: explicitly setting a value equal to the default
        // correctly overrides a prior non-default value.
        // system set log_level = "warn", then user explicitly sets it back to "info" (the default)
        // - old broken merge_from would have skipped it because "info" == default
        // - new apply_partial correctly sets it because the partial has Some("info")
        // (then project overrides to "debug", but the intermediate step passed)
        let mut s2 = Settings::default();
        let mut p1 = SettingsPartial::default();
        p1.general.log_level = Some("warn".to_string());
        s2.apply_partial(&p1);
        assert_eq!(s2.general.log_level, "warn");

        // Now explicitly reset to default value "info" - must work
        let mut p2 = SettingsPartial::default();
        p2.general.log_level = Some("info".to_string());
        s2.apply_partial(&p2);
        assert_eq!(s2.general.log_level, "info"); // Bug 5 would have left this as "warn"
    }

    #[test]
    fn test_settings_in_pitchfork_toml() {
        // Test parsing settings from pitchfork.toml format via SettingsPartial
        let toml_content = r#"
[daemons.myapp]
run = "node server.js"

[settings.general]
autostop_delay = "5m"
log_level = "debug"

[settings.web]
auto_start = true
bind_port = 8080
"#;

        // Parse the [settings] section as SettingsPartial
        let table: toml::Table = toml::from_str(toml_content).unwrap();
        let settings_table = table.get("settings").unwrap();
        let partial: SettingsPartial = settings_table.clone().try_into().unwrap();

        // Apply onto defaults to get resolved Settings
        let mut settings = Settings::default();
        settings.apply_partial(&partial);

        assert_eq!(settings.general.autostop_delay, "5m");
        assert_eq!(settings.general.log_level, "debug");
        assert!(settings.web.auto_start);
        assert_eq!(settings.web.bind_port, 8080);
        assert_eq!(settings.general.interval, "10s");
        assert_eq!(settings.ipc.connect_attempts, 5);

        // Unset fields in partial must be None
        assert!(partial.general.interval.is_none());
        assert!(partial.ipc.connect_attempts.is_none());
    }
}
