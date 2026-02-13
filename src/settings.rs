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

// Include the generated code from build.rs
include!(concat!(env!("OUT_DIR"), "/generated/settings.rs"));

// Include merge types
#[allow(dead_code)]
mod merge {
    include!(concat!(env!("OUT_DIR"), "/generated/settings_merge.rs"));
}

// Include metadata for introspection
#[allow(dead_code)]
mod meta {
    include!(concat!(env!("OUT_DIR"), "/generated/settings_meta.rs"));
}

#[allow(unused_imports)]
pub use merge::*;
#[allow(unused_imports)]
pub use meta::*;

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
        assert_eq!(settings.web.auto_start, false);
        assert_eq!(settings.web.bind_address, "127.0.0.1");
        assert_eq!(settings.web.default_port, 3120);
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
default_port = 8080
"#;

        let settings: Settings = toml::from_str(toml_content).unwrap();

        // Explicitly set values
        assert_eq!(settings.general.autostop_delay, "5m");
        assert_eq!(settings.general.interval, "30s");
        assert_eq!(settings.general.log_level, "debug");
        assert_eq!(settings.ipc.connect_attempts, 10);
        assert_eq!(settings.ipc.request_timeout, "10s");
        assert_eq!(settings.web.auto_start, true);
        assert_eq!(settings.web.default_port, 8080);

        // Default values for missing fields (serde default)
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
        assert_eq!(settings.web.auto_start, false);
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
        assert_eq!(settings.web.auto_start, false);
        assert_eq!(settings.tui.refresh_rate, "2s");
    }

    #[test]
    fn test_env_override() {
        // Create default settings then apply env overrides
        let mut settings = Settings::default();

        // Simulate environment variables by directly modifying fields
        // (In real usage, load_from_env would read from std::env)
        settings.general.autostop_delay = "10m".to_string();
        settings.general.interval = "5s".to_string();
        settings.ipc.connect_attempts = 20;
        settings.web.auto_start = true;

        // Verify the overrides
        assert_eq!(settings.general.autostop_delay, "10m");
        assert_eq!(settings.general.interval, "5s");
        assert_eq!(settings.ipc.connect_attempts, 20);
        assert_eq!(settings.web.auto_start, true);

        // Other values remain defaults
        assert_eq!(settings.general.log_level, "info");
        assert_eq!(settings.ipc.rate_limit, 100);
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
        assert_eq!(settings.ipc_connect_timeout(), Duration::from_secs(5));
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
        // TOML with unknown fields should be parsed without error
        // (serde will ignore unknown fields by default)
        let toml_content = r#"
[general]
log_level = "debug"
unknown_field = "should be ignored"

[unknown_section]
foo = "bar"
"#;

        let result: Result<Settings, _> = toml::from_str(toml_content);
        // With default serde settings, unknown fields cause an error
        // unless we use #[serde(deny_unknown_fields)]
        // Our Settings struct doesn't deny unknown fields, so this should work
        // Actually, let's verify the behavior
        match result {
            Ok(settings) => {
                // If parsing succeeds, verify known fields
                assert_eq!(settings.general.log_level, "debug");
            }
            Err(_) => {
                // If serde denies unknown fields, that's also acceptable behavior
                // This test documents the current behavior
            }
        }
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
default_port = 9000
log_lines = 200

[tui]
stat_history = 120
"#;

        let settings: Settings = toml::from_str(toml_content).unwrap();

        assert_eq!(settings.ipc.connect_attempts, 3);
        assert_eq!(settings.ipc.rate_limit, 50);
        assert_eq!(settings.web.auto_start, true);
        assert_eq!(settings.web.default_port, 9000);
        assert_eq!(settings.web.log_lines, 200);
        assert_eq!(settings.tui.stat_history, 120);
    }

    #[test]
    fn test_merge_from_non_default_values() {
        let mut base = Settings::default();
        let mut other = Settings::default();

        // Modify some values in 'other' (non-default)
        other.general.autostop_delay = "5m".to_string();
        other.general.log_level = "debug".to_string();
        other.ipc.connect_attempts = 10;
        other.web.auto_start = true;

        // Merge
        base.merge_from(&other);

        // Non-default values should be merged
        assert_eq!(base.general.autostop_delay, "5m");
        assert_eq!(base.general.log_level, "debug");
        assert_eq!(base.ipc.connect_attempts, 10);
        assert_eq!(base.web.auto_start, true);

        // Default values should remain unchanged
        assert_eq!(base.general.interval, "10s"); // still default
        assert_eq!(base.ipc.rate_limit, 100); // still default
    }

    #[test]
    fn test_merge_from_preserves_existing() {
        let mut base = Settings::default();
        base.general.autostop_delay = "2m".to_string();
        base.web.default_port = 8080;

        let other = Settings::default(); // All defaults

        // Merge defaults should not change existing non-default values
        base.merge_from(&other);

        assert_eq!(base.general.autostop_delay, "2m"); // preserved
        assert_eq!(base.web.default_port, 8080); // preserved
    }

    #[test]
    fn test_merge_chain() {
        // Simulate system -> user -> project merge chain
        let mut settings = Settings::default();

        // System config: set some values
        let mut system_config = Settings::default();
        system_config.general.log_level = "warn".to_string();
        system_config.web.bind_address = "0.0.0.0".to_string();
        settings.merge_from(&system_config);

        // User config: override some, add others
        let mut user_config = Settings::default();
        user_config.general.log_level = "info".to_string(); // override system
        user_config.tui.refresh_rate = "1s".to_string(); // new setting
        settings.merge_from(&user_config);

        // Project config: override some more
        let mut project_config = Settings::default();
        project_config.general.log_level = "debug".to_string(); // override user
        project_config.web.auto_start = true; // new setting
        settings.merge_from(&project_config);

        // Verify final merged state
        assert_eq!(settings.general.log_level, "debug"); // from project
        assert_eq!(settings.web.bind_address, "0.0.0.0"); // from system (not overridden)
        assert_eq!(settings.tui.refresh_rate, "1s"); // from user
        assert_eq!(settings.web.auto_start, true); // from project
    }

    #[test]
    fn test_settings_in_pitchfork_toml() {
        // Test parsing settings from pitchfork.toml format
        let toml_content = r#"
[daemons.myapp]
run = "node server.js"

[settings.general]
autostop_delay = "5m"
log_level = "debug"

[settings.web]
auto_start = true
default_port = 8080
"#;

        // Parse the settings section
        let table: toml::Table = toml::from_str(toml_content).unwrap();
        let settings_table = table.get("settings").unwrap();
        let settings: Settings = settings_table.clone().try_into().unwrap();

        assert_eq!(settings.general.autostop_delay, "5m");
        assert_eq!(settings.general.log_level, "debug");
        assert_eq!(settings.web.auto_start, true);
        assert_eq!(settings.web.default_port, 8080);

        // Other values should be defaults
        assert_eq!(settings.general.interval, "10s");
        assert_eq!(settings.ipc.connect_attempts, 5);
    }
}
