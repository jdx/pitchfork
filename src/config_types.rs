//! Configuration value types used across pitchfork.toml, daemon state, and CLI.
//!
//! These are thin wrappers (newtypes) around primitives with custom
//! serialization, validation, or display logic.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// StringOrStruct: serde "string or struct" pattern (bidirectional)
// ---------------------------------------------------------------------------

/// Trait for config types that accept either a string shorthand or a full object.
///
/// Follows the serde `string_or_struct` pattern, extended with serialization:
/// - Deserialize: string -> `from_short`, object -> deserialize `Raw` then `from_raw`
/// - Serialize: `is_shorthand` -> serialize `Short`, else -> `to_raw` then serialize `Raw`
///
/// Implementors provide 4 things:
/// - `Short` / `Raw` associated types (with serde derives)
/// - `from_short` / `from_raw` to construct Self
/// - `is_shorthand` / `to_short` / `to_raw` for serialization direction
pub trait StringOrStruct: Sized {
    type Short: for<'de> Deserialize<'de> + Serialize;
    type Raw: for<'de> Deserialize<'de> + Serialize;

    fn from_short(short: Self::Short) -> Self;
    fn from_raw(raw: Self::Raw) -> std::result::Result<Self, String>;
    fn is_shorthand(&self) -> bool;
    fn to_short(&self) -> Self::Short;
    fn to_raw(&self) -> Self::Raw;

    fn string_or_struct_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.is_shorthand() {
            self.to_short().serialize(s)
        } else {
            self.to_raw().serialize(s)
        }
    }

    fn string_or_struct_deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct Visitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: StringOrStruct> serde::de::Visitor<'de> for Visitor<T> {
            type Value = T;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or an object")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<T, E> {
                let short = T::Short::deserialize(serde::de::value::StrDeserializer::<E>::new(v))
                    .map_err(E::custom)?;
                Ok(T::from_short(short))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
                let raw = T::Raw::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                T::from_raw(raw).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(Visitor::<Self>(std::marker::PhantomData))
    }
}

// ---------------------------------------------------------------------------
// BoolOrU32 serde helpers
// ---------------------------------------------------------------------------

/// Trait for types that serialize as `u32` (or `bool` for the sentinel value)
/// and deserialize from either a boolean or a non-negative integer.
///
/// `true` maps to `TRUE_VALUE` (typically `u32::MAX`), `false` maps to 0.
///
/// Implementors only need to specify `TRUE_VALUE`; the `From<u32>` and
/// `Into<u32>` conversions are provided via derive_more or manual impls.
pub trait BoolOrU32: Sized + Copy + From<u32> + Into<u32> {
    const TRUE_VALUE: u32;

    fn bool_or_u32_serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let raw: u32 = (*self).into();
        if raw == Self::TRUE_VALUE {
            serializer.serialize_bool(true)
        } else {
            serializer.serialize_u32(raw)
        }
    }

    fn bool_or_u32_deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct Visitor<T>(std::marker::PhantomData<T>);

        impl<T: BoolOrU32> serde::de::Visitor<'_> for Visitor<T> {
            type Value = T;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a boolean or non-negative integer")
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<T, E> {
                Ok(T::from(if v { T::TRUE_VALUE } else { 0 }))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<T, E> {
                Ok(T::from(u32::try_from(v).unwrap_or(T::TRUE_VALUE)))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<T, E> {
                if v < 0 {
                    Err(E::custom("value cannot be negative"))
                } else {
                    self.visit_u64(v as u64)
                }
            }
        }

        deserializer.deserialize_any(Visitor::<Self>(std::marker::PhantomData))
    }
}

// ---------------------------------------------------------------------------
// Shared readiness timeout helpers
// ---------------------------------------------------------------------------

/// Parse a humantime duration string (e.g. "30s", "5m") into a `Duration`.
/// Returns `Ok(None)` when the input is `None`.
fn parse_timeout(raw: &Option<String>) -> std::result::Result<Option<std::time::Duration>, String> {
    raw.as_ref()
        .map(|s| humantime::parse_duration(s).map_err(|e| format!("invalid timeout: {e}")))
        .transpose()
}

/// Parse a humantime duration for an `interval` field. Same as `parse_timeout`
/// but reports the correct field name in the error.
fn parse_interval(
    raw: &Option<String>,
) -> std::result::Result<Option<std::time::Duration>, String> {
    raw.as_ref()
        .map(|s| humantime::parse_duration(s).map_err(|e| format!("invalid interval: {e}")))
        .transpose()
}

/// Format a `Duration` as a humantime string, or `None` when unset.
fn format_timeout(timeout: Option<std::time::Duration>) -> Option<String> {
    timeout.map(|d| humantime::format_duration(d).to_string())
}

// ---------------------------------------------------------------------------
// MemoryLimit
// ---------------------------------------------------------------------------

/// A byte-size type that accepts human-readable strings like "50MB", "1GiB", etc.
#[derive(Clone, Copy, PartialEq, Eq, humanbyte::HumanByte)]
pub struct MemoryLimit(pub u64);

// ---------------------------------------------------------------------------
// CpuLimit
// ---------------------------------------------------------------------------

/// CPU usage limit as a percentage (e.g. `80.0` = 80% of one core, `200.0` = 2 cores).
#[derive(
    Debug, Clone, Copy, PartialEq, Serialize, Deserialize, derive_more::Into, derive_more::Display,
)]
#[display("{}%", _0)]
#[into(f64)]
#[serde(try_from = "f64")]
pub struct CpuLimit(pub f32);

impl TryFrom<f64> for CpuLimit {
    type Error = String;

    fn try_from(v: f64) -> std::result::Result<Self, Self::Error> {
        if v <= 0.0 {
            return Err("cpu_limit must be positive".into());
        }
        Ok(Self(v as f32))
    }
}

impl JsonSchema for CpuLimit {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("CpuLimit")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "number",
            "description": "CPU usage limit as a percentage (e.g. 80 for 80% of one core, 200 for 2 cores)",
            "exclusiveMinimum": 0
        })
    }
}

// ---------------------------------------------------------------------------
// ReadyHttp
// ---------------------------------------------------------------------------

/// HTTP readiness check configuration.
///
/// Accepts two TOML forms:
/// ```toml
/// ready_http = "http://localhost:3000/health"  # shorthand, any 2xx response
/// ready_http = { url = "http://localhost:3000/health", status = [200, 401], timeout = "30s" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadyHttp {
    pub url: String,
    /// Exact status codes that indicate readiness. Empty means any 2xx response.
    pub status: Vec<u16>,
    /// Optional overall polling timeout. When set, the HTTP readiness check stops
    /// after this deadline and the daemon fails if no other check succeeds.
    pub timeout: Option<std::time::Duration>,
}

impl ReadyHttp {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            status: vec![],
            timeout: None,
        }
    }

    pub fn accepts_status(&self, status: u16) -> bool {
        if self.status.is_empty() {
            (200..=299).contains(&status)
        } else {
            self.status.contains(&status)
        }
    }
}

impl std::fmt::Display for ReadyHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status.is_empty() {
            f.write_str(&self.url)
        } else {
            let status = self
                .status
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            write!(f, "{} (status: {})", self.url, status)
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct ReadyHttpRaw {
    url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    status: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
}

impl StringOrStruct for ReadyHttp {
    type Short = String;
    type Raw = ReadyHttpRaw;

    fn from_short(url: String) -> Self {
        Self::new(url)
    }

    fn from_raw(raw: ReadyHttpRaw) -> std::result::Result<Self, String> {
        for status in &raw.status {
            if !(100..=599).contains(status) {
                return Err(format!(
                    "ready_http status must be between 100 and 599: {status}"
                ));
            }
        }
        let timeout = parse_timeout(&raw.timeout)?;
        Ok(Self {
            url: raw.url,
            status: raw.status,
            timeout,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.status.is_empty() && self.timeout.is_none()
    }

    fn to_short(&self) -> String {
        self.url.clone()
    }

    fn to_raw(&self) -> ReadyHttpRaw {
        ReadyHttpRaw {
            url: self.url.clone(),
            status: self.status.clone(),
            timeout: format_timeout(self.timeout),
        }
    }
}

impl Serialize for ReadyHttp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for ReadyHttp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

impl JsonSchema for ReadyHttp {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ReadyHttp")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "HTTP readiness check: a URL string accepting any 2xx response, or { url, status, timeout } object with exact accepted status codes and optional overall polling timeout",
            "oneOf": [
                { "type": "string", "description": "HTTP URL to poll for readiness; any 2xx response is ready" },
                {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "HTTP URL to poll for readiness" },
                        "status": {
                            "type": "array",
                            "description": "Exact HTTP status codes that indicate readiness. Omit to accept any 2xx response.",
                            "items": { "type": "integer", "minimum": 100, "maximum": 599 }
                        },
                        "timeout": { "type": "string", "description": "Overall readiness polling timeout (e.g. '30s', '5m'). Distinct from per-request http_client_timeout." }
                    },
                    "required": ["url"]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// ReadyCmd
// ---------------------------------------------------------------------------

/// Command readiness check configuration.
///
/// Accepts two TOML forms:
/// ```toml
/// ready_cmd = "pg_isready -h localhost"                        # shorthand, no timeout
/// ready_cmd = { run = "pg_isready -h localhost", timeout = "30s" } # full
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadyCmd {
    /// Shell command to run. Exit code 0 indicates readiness.
    pub run: String,
    /// Optional overall polling timeout. When set, the command readiness check stops
    /// after this deadline and the daemon fails if no other check succeeds.
    pub timeout: Option<std::time::Duration>,
}

impl ReadyCmd {
    pub fn new(run: impl Into<String>) -> Self {
        Self {
            run: run.into(),
            timeout: None,
        }
    }
}

impl std::fmt::Display for ReadyCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.run)
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct ReadyCmdRaw {
    run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
}

impl StringOrStruct for ReadyCmd {
    type Short = String;
    type Raw = ReadyCmdRaw;

    fn from_short(run: String) -> Self {
        Self::new(run)
    }

    fn from_raw(raw: ReadyCmdRaw) -> std::result::Result<Self, String> {
        let timeout = parse_timeout(&raw.timeout)?;
        Ok(Self {
            run: raw.run,
            timeout,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.timeout.is_none()
    }

    fn to_short(&self) -> String {
        self.run.clone()
    }

    fn to_raw(&self) -> ReadyCmdRaw {
        ReadyCmdRaw {
            run: self.run.clone(),
            timeout: format_timeout(self.timeout),
        }
    }
}

impl Serialize for ReadyCmd {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for ReadyCmd {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

impl JsonSchema for ReadyCmd {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ReadyCmd")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Command readiness check: a shell command string, or { run, timeout } object with an optional overall polling timeout",
            "oneOf": [
                { "type": "string", "description": "Shell command that returns exit code 0 when ready" },
                {
                    "type": "object",
                    "properties": {
                        "run": { "type": "string", "description": "Shell command that returns exit code 0 when ready" },
                        "timeout": { "type": "string", "description": "Overall readiness polling timeout (e.g. '30s', '5m')" }
                    },
                    "required": ["run"]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// HealthCmd
// ---------------------------------------------------------------------------

/// Command health check configuration.
/// TOML forms:
///   health_cmd = "openssl s_client -connect localhost:8443 -brief </dev/null"
///   health_cmd = { run = "...", interval = "10s", timeout = "10s", retries = 3 }
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthCmd {
    /// Shell command to run. Exit code 0 = healthy.
    pub run: String,
    /// Time between probes. None = the `supervisor.health_check_interval` setting (default 10s).
    pub interval: Option<std::time::Duration>,
    /// Per-probe timeout. None = the `supervisor.health_cmd_timeout` setting (default 10s).
    pub timeout: Option<std::time::Duration>,
    /// Consecutive failed probes before the daemon is killed. None = the `supervisor.health_check_retries` setting (default 3).
    pub retries: Option<u32>,
}

impl HealthCmd {
    pub fn new(run: impl Into<String>) -> Self {
        Self {
            run: run.into(),
            interval: None,
            timeout: None,
            retries: None,
        }
    }
}

impl std::fmt::Display for HealthCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.run)
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct HealthCmdRaw {
    run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retries: Option<u32>,
}

impl StringOrStruct for HealthCmd {
    type Short = String;
    type Raw = HealthCmdRaw;

    fn from_short(run: String) -> Self {
        Self::new(run)
    }

    fn from_raw(raw: HealthCmdRaw) -> std::result::Result<Self, String> {
        if let Some(retries) = raw.retries
            && retries < 1
        {
            return Err(format!("health_cmd retries must be >= 1: {retries}"));
        }
        let interval = parse_timeout(&raw.interval)?;
        let timeout = parse_timeout(&raw.timeout)?;
        Ok(Self {
            run: raw.run,
            interval,
            timeout,
            retries: raw.retries,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.interval.is_none() && self.timeout.is_none() && self.retries.is_none()
    }

    fn to_short(&self) -> String {
        self.run.clone()
    }

    fn to_raw(&self) -> HealthCmdRaw {
        HealthCmdRaw {
            run: self.run.clone(),
            interval: format_timeout(self.interval),
            timeout: format_timeout(self.timeout),
            retries: self.retries,
        }
    }
}

impl Serialize for HealthCmd {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for HealthCmd {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

impl JsonSchema for HealthCmd {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("HealthCmd")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Command health check: a shell command string, or { run, interval, timeout, retries } object with periodic probing",
            "oneOf": [
                { "type": "string", "description": "Shell command that returns exit code 0 when healthy" },
                {
                    "type": "object",
                    "properties": {
                        "run": { "type": "string", "description": "Shell command that returns exit code 0 when healthy" },
                        "interval": { "type": "string", "description": "Time between health probes (e.g. '10s', '5m'). Omit to use the `supervisor.health_check_interval` setting (default 10s)." },
                        "timeout": { "type": "string", "description": "Per-probe timeout (e.g. '10s', '5m'). Omit to use the `supervisor.health_cmd_timeout` setting (default 10s)." },
                        "retries": { "type": "integer", "minimum": 1, "description": "Consecutive failed probes before the daemon is killed. Omit to use the `supervisor.health_check_retries` setting (default 3)." }
                    },
                    "required": ["run"]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// HealthHttp
// ---------------------------------------------------------------------------

/// HTTP health check configuration.
/// TOML forms:
///   health_http = "http://localhost:3000/health"
///   health_http = { url = "...", status = [200], interval = "10s", timeout = "5s", retries = 3 }
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthHttp {
    pub url: String,
    /// Exact accepted status codes; empty = any 2xx.
    pub status: Vec<u16>,
    /// Time between probes. None = the `supervisor.health_check_interval` setting (default 10s).
    pub interval: Option<std::time::Duration>,
    /// Per-request timeout. None = the `supervisor.health_http_timeout` setting (default 5s).
    pub timeout: Option<std::time::Duration>,
    /// Consecutive failed probes before the daemon is killed. None = the `supervisor.health_check_retries` setting (default 3).
    pub retries: Option<u32>,
}

impl HealthHttp {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            status: vec![],
            interval: None,
            timeout: None,
            retries: None,
        }
    }
}

impl std::fmt::Display for HealthHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status.is_empty() {
            f.write_str(&self.url)
        } else {
            let status = self
                .status
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            write!(f, "{} (status: {})", self.url, status)
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct HealthHttpRaw {
    url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    status: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retries: Option<u32>,
}

impl StringOrStruct for HealthHttp {
    type Short = String;
    type Raw = HealthHttpRaw;

    fn from_short(url: String) -> Self {
        Self::new(url)
    }

    fn from_raw(raw: HealthHttpRaw) -> std::result::Result<Self, String> {
        for status in &raw.status {
            if !(100..=599).contains(status) {
                return Err(format!(
                    "health_http status must be between 100 and 599: {status}"
                ));
            }
        }
        if let Some(retries) = raw.retries
            && retries < 1
        {
            return Err(format!("health_http retries must be >= 1: {retries}"));
        }
        let interval = parse_timeout(&raw.interval)?;
        let timeout = parse_timeout(&raw.timeout)?;
        Ok(Self {
            url: raw.url,
            status: raw.status,
            interval,
            timeout,
            retries: raw.retries,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.status.is_empty()
            && self.interval.is_none()
            && self.timeout.is_none()
            && self.retries.is_none()
    }

    fn to_short(&self) -> String {
        self.url.clone()
    }

    fn to_raw(&self) -> HealthHttpRaw {
        HealthHttpRaw {
            url: self.url.clone(),
            status: self.status.clone(),
            interval: format_timeout(self.interval),
            timeout: format_timeout(self.timeout),
            retries: self.retries,
        }
    }
}

impl Serialize for HealthHttp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for HealthHttp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

impl JsonSchema for HealthHttp {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("HealthHttp")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "HTTP health check: a URL string accepting any 2xx response, or { url, status, interval, timeout, retries } object with periodic probing",
            "oneOf": [
                { "type": "string", "description": "HTTP URL to poll for health; any 2xx response is healthy" },
                {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "HTTP URL to poll for health" },
                        "status": {
                            "type": "array",
                            "description": "Exact HTTP status codes that indicate health. Omit to accept any 2xx response.",
                            "items": { "type": "integer", "minimum": 100, "maximum": 599 }
                        },
                        "interval": { "type": "string", "description": "Time between health probes (e.g. '10s', '5m'). Omit to use the `supervisor.health_check_interval` setting (default 10s)." },
                        "timeout": { "type": "string", "description": "Per-request timeout (e.g. '5s', '30s'). Omit to use the `supervisor.health_http_timeout` setting (default 5s)." },
                        "retries": { "type": "integer", "minimum": 1, "description": "Consecutive failed probes before the daemon is killed. Omit to use the `supervisor.health_check_retries` setting (default 3)." }
                    },
                    "required": ["url"]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// HealthPort
// ---------------------------------------------------------------------------

/// TCP port health check configuration.
/// TOML forms:
///   health_port = 8443                             # literal port
///   health_port = "{{ daemons.redis.port }}"       # template
///   health_port = { port = 8443, interval = "10s", retries = 3 }
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthPort {
    /// TCP port number to probe for health. `None` when the port is
    /// provided as a template.
    pub port: Option<u16>,
    /// Tera template string that renders to a port number. `None` when a
    /// literal port is used.
    pub template: Option<String>,
    /// Time between probes. None = the `supervisor.health_check_interval` setting (default 10s).
    pub interval: Option<std::time::Duration>,
    /// Consecutive failed probes before the daemon is killed. None = the `supervisor.health_check_retries` setting (default 3).
    pub retries: Option<u32>,
}

impl HealthPort {
    pub fn new(port: u16) -> Self {
        Self {
            port: Some(port),
            template: None,
            interval: None,
            retries: None,
        }
    }

    pub fn from_template(template: impl Into<String>) -> Self {
        Self {
            port: None,
            template: Some(template.into()),
            interval: None,
            retries: None,
        }
    }

    /// The resolved port number. `None` if this is an unrendered template.
    pub fn as_port(&self) -> Option<u16> {
        self.port
    }
}

impl From<u16> for HealthPort {
    fn from(port: u16) -> Self {
        Self::new(port)
    }
}

impl std::str::FromStr for HealthPort {
    type Err = String;

    /// Numeric strings must be a valid port (1-65535); anything else
    /// (non-empty) is treated as a Tera template.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            Err("health_port cannot be empty".to_string())
        } else if let Ok(n) = s.parse::<i64>() {
            u16::try_from(n)
                .ok()
                .filter(|&p| p > 0)
                .map(Self::new)
                .ok_or_else(|| format!("health_port out of range (1-65535): {n}"))
        } else {
            Ok(Self::from_template(s.to_string()))
        }
    }
}

impl std::fmt::Display for HealthPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.port, &self.template) {
            (Some(port), _) => write!(f, "{port}"),
            (None, Some(template)) => f.write_str(template),
            (None, None) => Ok(()),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct HealthPortRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retries: Option<u32>,
}

impl Serialize for HealthPort {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.interval.is_none() && self.retries.is_none() {
            if let Some(port) = self.port {
                return s.serialize_u16(port);
            }
            if let Some(template) = &self.template {
                return s.serialize_str(template);
            }
            return s.serialize_none();
        }
        HealthPortRaw {
            port: self.port,
            template: self.template.clone(),
            interval: format_timeout(self.interval),
            retries: self.retries,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for HealthPort {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;

        impl<'de> serde::de::Visitor<'de> for V {
            type Value = HealthPort;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "a port number, a template string, or an object with 'port'/'template' and optional 'interval'/'retries'",
                )
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                u16::try_from(v)
                    .ok()
                    .filter(|&p| p > 0)
                    .map(HealthPort::new)
                    .ok_or_else(|| E::custom(format!("health_port out of range (1-65535): {v}")))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                if v < 0 {
                    Err(E::custom("health_port cannot be negative"))
                } else {
                    self.visit_u64(v as u64)
                }
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse().map_err(E::custom)
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                let raw =
                    HealthPortRaw::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                if raw.port.is_none() && raw.template.is_none() {
                    return Err(serde::de::Error::custom(
                        "health_port object must have either 'port' or 'template'",
                    ));
                }
                if let Some(port) = raw.port
                    && port == 0
                {
                    return Err(serde::de::Error::custom("port must be between 1 and 65535"));
                }
                if let Some(retries) = raw.retries
                    && retries < 1
                {
                    return Err(serde::de::Error::custom(format!(
                        "health_port retries must be >= 1: {retries}"
                    )));
                }
                let interval = parse_interval(&raw.interval).map_err(serde::de::Error::custom)?;
                Ok(HealthPort {
                    port: raw.port,
                    template: raw.template,
                    interval,
                    retries: raw.retries,
                })
            }
        }

        d.deserialize_any(V)
    }
}

impl JsonSchema for HealthPort {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("HealthPort")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "TCP port health check: a port number, a template string rendering to one, or a { port, interval, retries } object with periodic probing",
            "oneOf": [
                { "type": "integer", "minimum": 1, "maximum": 65535, "description": "TCP port number to probe for health" },
                { "type": "string", "description": "Tera template that renders to a port number" },
                {
                    "type": "object",
                    "properties": {
                        "port": { "type": "integer", "minimum": 1, "maximum": 65535, "description": "TCP port number to probe for health" },
                        "template": { "type": "string", "description": "Tera template that renders to a port number" },
                        "interval": { "type": "string", "description": "Time between health probes (e.g. '10s', '5m'). Omit to use the `supervisor.health_check_interval` setting (default 10s)." },
                        "retries": { "type": "integer", "minimum": 1, "description": "Consecutive failed probes before the daemon is killed. Omit to use the `supervisor.health_check_retries` setting (default 3)." }
                    },
                    "oneOf": [
                        { "required": ["port"] },
                        { "required": ["template"] }
                    ]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// ReadyPort
// ---------------------------------------------------------------------------

/// TCP readiness port: a literal port number or a Tera template string that
/// renders to one, plus an optional overall polling timeout.
///
/// Accepts four TOML forms:
/// ```toml
/// ready_port = 3000                                  # literal port
/// ready_port = "{{ daemons.redis.port }}"            # template
/// ready_port = { port = 3000, timeout = "30s" }      # literal with timeout
/// ready_port = { template = "{{ daemons.redis.port }}", timeout = "30s" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadyPort {
    /// TCP port number to check for readiness. `None` when the port is
    /// provided as a template.
    pub port: Option<u16>,
    /// Tera template string that renders to a port number. `None` when a
    /// literal port is used.
    pub template: Option<String>,
    /// Optional overall polling timeout. When set, the port readiness check stops
    /// after this deadline and the daemon fails if no other check succeeds.
    pub timeout: Option<std::time::Duration>,
}

impl ReadyPort {
    pub fn new(port: u16) -> Self {
        Self {
            port: Some(port),
            template: None,
            timeout: None,
        }
    }

    pub fn from_template(template: impl Into<String>) -> Self {
        Self {
            port: None,
            template: Some(template.into()),
            timeout: None,
        }
    }

    /// The resolved port number. `None` if this is an unrendered template.
    pub fn as_port(&self) -> Option<u16> {
        self.port
    }
}

impl From<u16> for ReadyPort {
    fn from(port: u16) -> Self {
        Self::new(port)
    }
}

impl std::str::FromStr for ReadyPort {
    type Err = String;

    /// Numeric strings must be a valid port (1-65535); anything else
    /// (non-empty) is treated as a Tera template.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            Err("ready_port cannot be empty".to_string())
        } else if let Ok(n) = s.parse::<i64>() {
            u16::try_from(n)
                .ok()
                .filter(|&p| p > 0)
                .map(Self::new)
                .ok_or_else(|| format!("ready_port out of range (1-65535): {n}"))
        } else {
            Ok(Self::from_template(s.to_string()))
        }
    }
}

impl std::fmt::Display for ReadyPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.port, &self.template) {
            (Some(port), _) => write!(f, "{port}"),
            (None, Some(template)) => f.write_str(template),
            (None, None) => Ok(()),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct ReadyPortRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
}

impl Serialize for ReadyPort {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.timeout.is_none() {
            if let Some(port) = self.port {
                return s.serialize_u16(port);
            }
            if let Some(template) = &self.template {
                return s.serialize_str(template);
            }
            return s.serialize_none();
        }
        ReadyPortRaw {
            port: self.port,
            template: self.template.clone(),
            timeout: format_timeout(self.timeout),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for ReadyPort {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;

        impl<'de> serde::de::Visitor<'de> for V {
            type Value = ReadyPort;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "a port number, a template string, or an object with 'port'/'template' and optional 'timeout'",
                )
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                u16::try_from(v)
                    .ok()
                    .filter(|&p| p > 0)
                    .map(ReadyPort::new)
                    .ok_or_else(|| E::custom(format!("ready_port out of range (1-65535): {v}")))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                if v < 0 {
                    Err(E::custom("ready_port cannot be negative"))
                } else {
                    self.visit_u64(v as u64)
                }
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse().map_err(E::custom)
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                let raw =
                    ReadyPortRaw::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                if raw.port.is_none() && raw.template.is_none() {
                    return Err(serde::de::Error::custom(
                        "ready_port object must have either 'port' or 'template'",
                    ));
                }
                if let Some(port) = raw.port
                    && port == 0
                {
                    return Err(serde::de::Error::custom("port must be between 1 and 65535"));
                }
                let timeout = parse_timeout(&raw.timeout).map_err(serde::de::Error::custom)?;
                Ok(ReadyPort {
                    port: raw.port,
                    template: raw.template,
                    timeout,
                })
            }
        }

        d.deserialize_any(V)
    }
}

impl JsonSchema for ReadyPort {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ReadyPort")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "TCP readiness port: a port number, a template string rendering to one, or an object with an optional overall polling timeout",
            "oneOf": [
                { "type": "integer", "minimum": 1, "maximum": 65535, "description": "TCP port number to check for readiness" },
                { "type": "string", "description": "Tera template that renders to a port number" },
                {
                    "type": "object",
                    "properties": {
                        "port": { "type": "integer", "minimum": 1, "maximum": 65535, "description": "TCP port number to check for readiness" },
                        "template": { "type": "string", "description": "Tera template that renders to a port number" },
                        "timeout": { "type": "string", "description": "Overall readiness polling timeout (e.g. '30s', '5m')" }
                    },
                    "oneOf": [
                        { "required": ["port"] },
                        { "required": ["template"] }
                    ]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// ReadyOutput
// ---------------------------------------------------------------------------

/// Output readiness check configuration.
///
/// Accepts two TOML forms:
/// ```toml
/// ready_output = "Server listening"                   # shorthand, no timeout
/// ready_output = { pattern = "Server listening", timeout = "30s" }  # full
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadyOutput {
    /// Regex pattern matched against ANSI-stripped stdout/stderr lines.
    pub pattern: String,
    /// Optional overall polling timeout. When set, the output readiness check stops
    /// after this deadline and the daemon fails if no other check succeeds.
    pub timeout: Option<std::time::Duration>,
}

impl ReadyOutput {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            timeout: None,
        }
    }
}

impl std::fmt::Display for ReadyOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.pattern)
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct ReadyOutputRaw {
    pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
}

impl StringOrStruct for ReadyOutput {
    type Short = String;
    type Raw = ReadyOutputRaw;

    fn from_short(pattern: String) -> Self {
        Self::new(pattern)
    }

    fn from_raw(raw: ReadyOutputRaw) -> std::result::Result<Self, String> {
        let timeout = parse_timeout(&raw.timeout)?;
        Ok(Self {
            pattern: raw.pattern,
            timeout,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.timeout.is_none()
    }

    fn to_short(&self) -> String {
        self.pattern.clone()
    }

    fn to_raw(&self) -> ReadyOutputRaw {
        ReadyOutputRaw {
            pattern: self.pattern.clone(),
            timeout: format_timeout(self.timeout),
        }
    }
}

impl Serialize for ReadyOutput {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for ReadyOutput {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

impl JsonSchema for ReadyOutput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ReadyOutput")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Output readiness check: a regex pattern string, or { pattern, timeout } object with an optional overall polling timeout",
            "oneOf": [
                { "type": "string", "description": "Regex pattern matched against ANSI-stripped stdout/stderr lines" },
                {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regex pattern matched against ANSI-stripped stdout/stderr lines" },
                        "timeout": { "type": "string", "description": "Overall readiness polling timeout (e.g. '30s', '5m')" }
                    },
                    "required": ["pattern"]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// StopSignal
// ---------------------------------------------------------------------------

/// Unix signal for graceful daemon shutdown (the first signal sent before SIGKILL).
///
/// Accepts signal names with or without `SIG` prefix, case-insensitive:
/// `"SIGINT"`, `"INT"`, `"sigint"` are all equivalent.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Deserialize, derive_more::Into, derive_more::Display,
)]
#[display("SIG{}", self.name())]
#[into(i32)]
#[serde(try_from = "String")]
pub struct StopSignal(i32);

// Signal numbers. On Unix we pull them from `libc` so they match the platform
// (SIGUSR1/2 differ between Linux and BSD/macOS). On Windows the values are
// only used for parsing and Display — `procs::kill` ignores `stop_signal` and
// uses TerminateProcess via sysinfo — so POSIX-typical Linux values are fine.
#[cfg(unix)]
use libc::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGUSR1, SIGUSR2};
#[cfg(windows)]
const SIGHUP: i32 = 1;
#[cfg(windows)]
const SIGINT: i32 = 2;
#[cfg(windows)]
const SIGQUIT: i32 = 3;
#[cfg(windows)]
const SIGTERM: i32 = 15;
#[cfg(windows)]
const SIGUSR1: i32 = 10;
#[cfg(windows)]
const SIGUSR2: i32 = 12;

const SIGNAL_TABLE: &[(&str, i32)] = &[
    ("HUP", SIGHUP),
    ("INT", SIGINT),
    ("QUIT", SIGQUIT),
    ("TERM", SIGTERM),
    ("USR1", SIGUSR1),
    ("USR2", SIGUSR2),
];

impl StopSignal {
    pub fn name(self) -> &'static str {
        SIGNAL_TABLE
            .iter()
            .find(|(_, sig)| *sig == self.0)
            .map(|(name, _)| *name)
            .unwrap_or("UNKNOWN")
    }
}

impl Default for StopSignal {
    fn default() -> Self {
        Self(SIGTERM)
    }
}

impl TryFrom<String> for StopSignal {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        let upper = s.trim().to_ascii_uppercase();
        let name = upper.strip_prefix("SIG").unwrap_or(&upper);
        SIGNAL_TABLE
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, sig)| Self(*sig))
            .ok_or_else(|| format!("unsupported stop signal: {s}"))
    }
}

impl Serialize for StopSignal {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl JsonSchema for StopSignal {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("StopSignal")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Unix signal for graceful shutdown (e.g. 'SIGTERM', 'SIGINT', 'SIGHUP')",
            "enum": ["SIGTERM", "SIGINT", "SIGQUIT", "SIGHUP", "SIGUSR1", "SIGUSR2"]
        })
    }
}

// ---------------------------------------------------------------------------
// StopConfig (string-or-object pattern)
// ---------------------------------------------------------------------------

/// Daemon stop configuration: a signal and an optional per-daemon timeout.
///
/// Accepts two TOML forms:
/// ```toml
/// stop_signal = "SIGINT"                         # shorthand
/// stop_signal = { signal = "SIGINT", timeout = "500ms" }  # full
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StopConfig {
    pub signal: StopSignal,
    pub timeout: Option<std::time::Duration>,
}

/// Helper for the object form of StopConfig.
#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct StopConfigRaw {
    signal: StopSignal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
}

impl StringOrStruct for StopConfig {
    type Short = StopSignal;
    type Raw = StopConfigRaw;

    fn from_short(signal: StopSignal) -> Self {
        Self {
            signal,
            timeout: None,
        }
    }

    fn from_raw(raw: StopConfigRaw) -> std::result::Result<Self, String> {
        let timeout = parse_timeout(&raw.timeout)?;
        Ok(Self {
            signal: raw.signal,
            timeout,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.timeout.is_none()
    }

    fn to_short(&self) -> StopSignal {
        self.signal
    }

    fn to_raw(&self) -> StopConfigRaw {
        StopConfigRaw {
            signal: self.signal,
            timeout: format_timeout(self.timeout),
        }
    }
}

impl Serialize for StopConfig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for StopConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

impl JsonSchema for StopConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("StopConfig")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Stop signal config: a signal name string, or { signal, timeout } object",
            "oneOf": [
                generator.subschema_for::<StopSignal>(),
                {
                    "type": "object",
                    "properties": {
                        "signal": generator.subschema_for::<StopSignal>(),
                        "timeout": { "type": "string", "description": "Graceful shutdown timeout (e.g. '500ms', '3s')" }
                    },
                    "required": ["signal"]
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// Retry
// ---------------------------------------------------------------------------

/// Retry configuration: `true` = indefinite, `false`/`0` = none, number = count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, derive_more::From, derive_more::Into)]
pub struct Retry(pub u32);

impl BoolOrU32 for Retry {
    const TRUE_VALUE: u32 = u32::MAX;
}

impl Retry {
    pub const INFINITE: Retry = Retry(u32::MAX);
    pub fn count(&self) -> u32 {
        self.0
    }
    pub fn is_infinite(&self) -> bool {
        self.0 == u32::MAX
    }
}

impl std::fmt::Display for Retry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_infinite() {
            f.write_str("infinite")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl Serialize for Retry {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.bool_or_u32_serialize(s)
    }
}

impl<'de> Deserialize<'de> for Retry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::bool_or_u32_deserialize(d)
    }
}

impl JsonSchema for Retry {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Retry")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Retry: true = indefinite, false/0 = none, number = count",
            "oneOf": [
                { "type": "boolean" },
                { "type": "integer", "minimum": 0 }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// WatchMode
// ---------------------------------------------------------------------------

/// File watch backend mode for daemon `watch` patterns.
#[derive(
    Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WatchMode {
    /// Use platform-native watcher backend (inotify/FSEvents/ReadDirectoryChangesW).
    #[default]
    Native,
    /// Use polling backend; more compatible on networked filesystems.
    Poll,
    /// Prefer native backend, fall back to polling when native watch setup fails.
    Auto,
}

// ---------------------------------------------------------------------------
// CronRetrigger
// ---------------------------------------------------------------------------

/// Retrigger behavior for cron-scheduled daemons
#[derive(
    Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CronRetrigger {
    /// Retrigger only if the previous run has finished (success or error)
    #[default]
    Finish,
    /// Always retrigger, stopping the previous run if still active
    Always,
    /// Retrigger only if the previous run succeeded
    Success,
    /// Retrigger only if the previous run failed
    Fail,
}

// PitchforkTomlCron (string-or-object pattern)
// ---------------------------------------------------------------------------

/// Cron scheduling configuration.
///
/// Accepts two forms:
/// ```toml
/// cron = "0 * * * *"                                    # shorthand
/// cron = { schedule = "0 * * * *", retrigger = "always" }  # full
/// ```
#[derive(Debug, Clone)]
pub struct PitchforkTomlCron {
    /// Cron expression (e.g., '0 * * * *' for hourly, '*/5 * * * *' for every 5 minutes)
    pub schedule: String,
    /// Behavior when cron triggers while previous run is still active
    pub retrigger: CronRetrigger,
    /// Whether to trigger immediately on first check when no prior trigger is recorded.
    /// When false (default), the first trigger is deferred until the next scheduled time.
    pub immediate: bool,
}

impl JsonSchema for PitchforkTomlCron {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("PitchforkTomlCron")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Cron scheduling: a cron expression string, or { schedule, retrigger, immediate } object",
            "oneOf": [
                { "type": "string", "description": "Cron expression (e.g. '0 * * * *')" },
                {
                    "type": "object",
                    "properties": {
                        "schedule": { "type": "string", "description": "Cron expression" },
                        "retrigger": generator.subschema_for::<CronRetrigger>(),
                        "immediate": { "type": "boolean", "description": "Trigger immediately on first check (default: false)" }
                    },
                    "required": ["schedule"]
                }
            ]
        })
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct PitchforkTomlCronRaw {
    schedule: String,
    #[serde(default)]
    retrigger: CronRetrigger,
    #[serde(default)]
    immediate: bool,
}

impl StringOrStruct for PitchforkTomlCron {
    type Short = String;
    type Raw = PitchforkTomlCronRaw;

    fn from_short(schedule: String) -> Self {
        Self {
            schedule,
            retrigger: CronRetrigger::default(),
            immediate: false,
        }
    }

    fn from_raw(raw: PitchforkTomlCronRaw) -> std::result::Result<Self, String> {
        Ok(Self {
            schedule: raw.schedule,
            retrigger: raw.retrigger,
            immediate: raw.immediate,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.retrigger == CronRetrigger::default() && !self.immediate
    }

    fn to_short(&self) -> String {
        self.schedule.clone()
    }

    fn to_raw(&self) -> PitchforkTomlCronRaw {
        PitchforkTomlCronRaw {
            schedule: self.schedule.clone(),
            retrigger: self.retrigger,
            immediate: self.immediate,
        }
    }
}

impl Serialize for PitchforkTomlCron {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for PitchforkTomlCron {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

// ---------------------------------------------------------------------------
// PitchforkTomlAuto
// ---------------------------------------------------------------------------

/// Auto start/stop configuration
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PitchforkTomlAuto {
    Start,
    Stop,
}

// ---------------------------------------------------------------------------
// OnOutputHook (string-or-object pattern)
// ---------------------------------------------------------------------------

/// Output hook configuration.
///
/// Accepts two forms:
/// ```toml
/// on_output = "echo matched"                              # shorthand (run only)
/// on_output = { run = "echo matched", filter = "ready" }  # full
/// ```
///
/// Pattern matching (`filter` / `regex`) is performed against ANSI-stripped
/// output and covers both stdout and stderr.
#[derive(Debug, Clone, JsonSchema)]
pub struct OnOutputHook {
    /// Command to run when the output condition is met
    pub run: String,
    /// Fire when a line of output contains this substring
    pub filter: Option<String>,
    /// Fire when a line of output matches this regular expression
    pub regex: Option<String>,
    /// Minimum time between successive firings (humantime, e.g. `"500ms"`).
    /// Defaults to `"1000ms"`.
    pub debounce: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct OnOutputHookRaw {
    run: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    regex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    debounce: Option<String>,
}

impl StringOrStruct for OnOutputHook {
    type Short = String;
    type Raw = OnOutputHookRaw;

    fn from_short(run: String) -> Self {
        Self {
            run,
            filter: None,
            regex: None,
            debounce: None,
        }
    }

    fn from_raw(raw: OnOutputHookRaw) -> std::result::Result<Self, String> {
        Ok(Self {
            run: raw.run,
            filter: raw.filter,
            regex: raw.regex,
            debounce: raw.debounce,
        })
    }

    fn is_shorthand(&self) -> bool {
        self.filter.is_none() && self.regex.is_none() && self.debounce.is_none()
    }

    fn to_short(&self) -> String {
        self.run.clone()
    }

    fn to_raw(&self) -> OnOutputHookRaw {
        OnOutputHookRaw {
            run: self.run.clone(),
            filter: self.filter.clone(),
            regex: self.regex.clone(),
            debounce: self.debounce.clone(),
        }
    }
}

impl Serialize for OnOutputHook {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.string_or_struct_serialize(s)
    }
}

impl<'de> Deserialize<'de> for OnOutputHook {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::string_or_struct_deserialize(d)
    }
}

impl OnOutputHook {
    /// Validate configuration: `filter` and `regex` are mutually exclusive,
    /// `regex` must be a valid regular expression, and `debounce` (if present)
    /// must be a valid humantime duration.
    pub fn validate(&self, daemon_name: &str) -> crate::Result<()> {
        if self.filter.is_some() && self.regex.is_some() {
            miette::bail!(
                "daemon {daemon_name}: on_output.filter and on_output.regex are mutually exclusive"
            );
        }
        if let Some(ref pattern) = self.regex {
            regex::Regex::new(pattern).map_err(|e| {
                miette::miette!(
                    "daemon {daemon_name}: on_output.regex {pattern:?} is not a valid regular expression: {e}"
                )
            })?;
        }
        if let Some(ref d) = self.debounce {
            humantime::parse_duration(d).map_err(|e| {
                miette::miette!(
                    "daemon {daemon_name}: on_output.debounce {d:?} is not a valid duration: {e}"
                )
            })?;
        }
        Ok(())
    }

    /// Resolved debounce duration. Falls back to 1 second.
    pub fn debounce_duration(&self) -> std::time::Duration {
        self.debounce
            .as_deref()
            .and_then(|s| humantime::parse_duration(s).ok())
            .unwrap_or(std::time::Duration::from_millis(1000))
    }
}

// ---------------------------------------------------------------------------
// PitchforkTomlHooks
// ---------------------------------------------------------------------------

/// Lifecycle hooks for a daemon
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct PitchforkTomlHooks {
    /// Command to run when the daemon becomes ready
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_ready: Option<String>,
    /// Command to run when the daemon fails and all retries are exhausted
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_fail: Option<String>,
    /// Command to run before each retry attempt
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_retry: Option<String>,
    /// Command to run when the daemon is explicitly stopped by pitchfork
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_stop: Option<String>,
    /// Command to run on any daemon termination (clean exit, crash, or stop)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_exit: Option<String>,
    /// Hook triggered when the daemon produces matching output
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_output: Option<OnOutputHook>,
}

// ---------------------------------------------------------------------------
// PortBump (BoolOrU32 pattern)
// ---------------------------------------------------------------------------

/// Port bump attempts: `true` = unlimited, `false`/`0` = disabled, number = max attempts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, derive_more::From, derive_more::Into)]
pub struct PortBump(pub u32);

impl BoolOrU32 for PortBump {
    const TRUE_VALUE: u32 = u32::MAX;
}

impl Serialize for PortBump {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.bool_or_u32_serialize(s)
    }
}

impl<'de> Deserialize<'de> for PortBump {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::bool_or_u32_deserialize(d)
    }
}

impl JsonSchema for PortBump {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("PortBump")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Port bump: true = unlimited, false/0 = disabled, number = max attempts",
            "oneOf": [
                { "type": "boolean" },
                { "type": "integer", "minimum": 0 }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// PortConfig (number, array, or object)
// ---------------------------------------------------------------------------

/// Port configuration for a daemon.
///
/// Accepts three TOML forms:
/// ```toml
/// port = 5173                                  # single port
/// port = [5173, 5174]                          # multiple ports
/// port = { expect = [5173], bump = 10 }        # full form with bump
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PortConfig {
    pub expect: Vec<u16>,
    pub bump: PortBump,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[doc(hidden)]
pub struct PortConfigRaw {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expect: Vec<u16>,
    #[serde(default)]
    pub bump: PortBump,
}

impl Serialize for PortConfig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.bump.0 == 0 {
            if self.expect.len() == 1 {
                s.serialize_u16(self.expect[0])
            } else {
                self.expect.serialize(s)
            }
        } else {
            PortConfigRaw {
                expect: self.expect.clone(),
                bump: self.bump,
            }
            .serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for PortConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;

        impl<'de> serde::de::Visitor<'de> for V {
            type Value = PortConfig;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a port number, array of ports, or { expect, bump } object")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<PortConfig, E> {
                let port = u16::try_from(v)
                    .map_err(|_| E::custom(format!("port {v} out of range (0-65535)")))?;
                Ok(PortConfig {
                    expect: vec![port],
                    bump: PortBump(0),
                })
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<PortConfig, E> {
                if v < 0 {
                    Err(E::custom("port cannot be negative"))
                } else {
                    self.visit_u64(v as u64)
                }
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<PortConfig, A::Error> {
                let mut ports = Vec::new();
                while let Some(port) = seq.next_element::<u16>()? {
                    ports.push(port);
                }
                Ok(PortConfig {
                    expect: ports,
                    bump: PortBump(0),
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<PortConfig, A::Error> {
                let raw: PortConfigRaw =
                    Deserialize::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(PortConfig {
                    expect: raw.expect,
                    bump: raw.bump,
                })
            }
        }

        deserializer.deserialize_any(V)
    }
}

impl PortConfig {
    /// Construct from expected ports and bump config, returning `None` if both are empty/zero.
    pub fn from_parts(expect: Vec<u16>, bump: PortBump) -> Option<Self> {
        if expect.is_empty() && bump.0 == 0 {
            None
        } else {
            Some(Self { expect, bump })
        }
    }

    /// Whether auto-bump is enabled (bump > 0).
    pub fn auto_bump(&self) -> bool {
        self.bump.0 > 0
    }

    /// Maximum bump attempts. Returns 0 if bump is disabled.
    pub fn max_bump_attempts(&self) -> u32 {
        self.bump.0
    }
}

impl JsonSchema for PortConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("PortConfig")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Port config: a port number, array of ports, or { expect, bump } object",
            "oneOf": [
                { "type": "integer", "minimum": 0, "maximum": 65535 },
                { "type": "array", "items": { "type": "integer", "minimum": 0, "maximum": 65535 } },
                {
                    "type": "object",
                    "properties": {
                        "expect": { "type": "array", "items": { "type": "integer", "minimum": 0, "maximum": 65535 } },
                        "bump": generator.subschema_for::<PortBump>()
                    }
                }
            ]
        })
    }
}

// ---------------------------------------------------------------------------
// Dir (working directory with CWD default)
// ---------------------------------------------------------------------------

/// Working directory for a daemon process.
///
/// Defaults to the current working directory at process start.
#[derive(
    Debug,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    derive_more::From,
    derive_more::Into,
    derive_more::Deref,
    derive_more::AsRef,
)]
#[serde(transparent)]
#[deref(forward)]
#[as_ref(forward)]
pub struct Dir(pub std::path::PathBuf);

impl Default for Dir {
    fn default() -> Self {
        Self(crate::env::CWD.clone())
    }
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Minimal wrapper exercising the same TOML shape as `[daemons.<name>]`.
    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    struct TestDaemon {
        health_cmd: Option<HealthCmd>,
        health_http: Option<HealthHttp>,
        health_port: Option<HealthPort>,
    }

    #[test]
    fn test_health_cmd_string_form() {
        let daemon: TestDaemon = toml::from_str(
            "health_cmd = 'openssl s_client -connect localhost:8443 -brief </dev/null'",
        )
        .unwrap();
        let hc = daemon.health_cmd.unwrap();
        assert_eq!(
            hc.run,
            "openssl s_client -connect localhost:8443 -brief </dev/null"
        );
        assert!(hc.interval.is_none());
        assert!(hc.timeout.is_none());
        assert!(hc.retries.is_none());
    }

    #[test]
    fn test_health_cmd_object_form() {
        let daemon: TestDaemon = toml::from_str(
            r#"
health_cmd = { run = "curl -f http://localhost:3000/health", interval = "10s", timeout = "10s", retries = 3 }
"#,
        )
        .unwrap();
        let hc = daemon.health_cmd.unwrap();
        assert_eq!(hc.run, "curl -f http://localhost:3000/health");
        assert_eq!(hc.interval, Some(Duration::from_secs(10)));
        assert_eq!(hc.timeout, Some(Duration::from_secs(10)));
        assert_eq!(hc.retries, Some(3));
    }

    #[test]
    fn test_health_http_string_form() {
        let daemon: TestDaemon =
            toml::from_str(r#"health_http = "http://localhost:3000/health""#).unwrap();
        let hh = daemon.health_http.unwrap();
        assert_eq!(hh.url, "http://localhost:3000/health");
        assert!(hh.status.is_empty());
        assert!(hh.interval.is_none());
        assert!(hh.timeout.is_none());
        assert!(hh.retries.is_none());
    }

    #[test]
    fn test_health_http_object_form() {
        let daemon: TestDaemon = toml::from_str(
            r#"
health_http = { url = "http://localhost:3000/health", status = [200, 401], interval = "10s", timeout = "5s", retries = 3 }
"#,
        )
        .unwrap();
        let hh = daemon.health_http.unwrap();
        assert_eq!(hh.url, "http://localhost:3000/health");
        assert_eq!(hh.status, vec![200, 401]);
        assert_eq!(hh.interval, Some(Duration::from_secs(10)));
        assert_eq!(hh.timeout, Some(Duration::from_secs(5)));
        assert_eq!(hh.retries, Some(3));
    }

    #[test]
    fn test_health_serialize_roundtrip() {
        let daemon = TestDaemon {
            health_cmd: Some(HealthCmd {
                run: "curl -f http://localhost:3000/health".to_string(),
                interval: Some(Duration::from_secs(5)),
                timeout: None,
                retries: Some(2),
            }),
            health_http: Some(HealthHttp {
                url: "http://localhost:3000/health".to_string(),
                status: vec![200],
                interval: None,
                timeout: Some(Duration::from_secs(5)),
                retries: Some(2),
            }),
            health_port: None,
        };
        let serialized = toml::to_string(&daemon).unwrap();
        let back: TestDaemon = toml::from_str(&serialized).unwrap();
        assert_eq!(back.health_cmd, daemon.health_cmd);
        assert_eq!(back.health_http, daemon.health_http);
    }

    #[test]
    fn test_health_serialize_shorthand_roundtrip() {
        let daemon = TestDaemon {
            health_cmd: Some(HealthCmd::new("pg_isready -h localhost")),
            health_http: Some(HealthHttp::new("http://localhost:3000/health")),
            health_port: None,
        };
        let serialized = toml::to_string(&daemon).unwrap();
        assert!(serialized.contains("health_cmd = \"pg_isready -h localhost\""));
        assert!(serialized.contains("health_http = \"http://localhost:3000/health\""));
        let back: TestDaemon = toml::from_str(&serialized).unwrap();
        assert_eq!(back.health_cmd, daemon.health_cmd);
        assert_eq!(back.health_http, daemon.health_http);
    }

    #[test]
    fn test_health_port_object_missing_port_and_template_rejected() {
        let err = toml::from_str::<TestDaemon>("health_port = { retries = 2 }").unwrap_err();
        let err = format!("{err:?}");
        assert!(
            err.contains("health_port object must have either 'port' or 'template'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_http_invalid_status_rejected() {
        let err = toml::from_str::<TestDaemon>(
            r#"
health_http = { url = "http://localhost:3000/health", status = [99] }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("health_http status must be between 100 and 599"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_cmd_invalid_timeout_rejected() {
        let err = toml::from_str::<TestDaemon>(
            r#"
health_cmd = { run = "pg_isready", timeout = "not-a-duration" }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid timeout"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_http_invalid_timeout_rejected() {
        let err = toml::from_str::<TestDaemon>(
            r#"
health_http = { url = "http://localhost:3000/health", timeout = "not-a-duration" }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid timeout"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_cmd_retries_zero_rejected() {
        let err = toml::from_str::<TestDaemon>(
            r#"
health_cmd = { run = "pg_isready", retries = 0 }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("health_cmd retries must be >= 1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_http_retries_zero_rejected() {
        let err = toml::from_str::<TestDaemon>(
            r#"
health_http = { url = "http://localhost:3000/health", retries = 0 }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("health_http retries must be >= 1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_port_literal_form() {
        let daemon: TestDaemon = toml::from_str("health_port = 8443").unwrap();
        let hp = daemon.health_port.unwrap();
        assert_eq!(hp.port, Some(8443));
        assert!(hp.template.is_none());
        assert!(hp.interval.is_none());
        assert!(hp.retries.is_none());
    }

    #[test]
    fn test_health_port_template_string_form() {
        let daemon: TestDaemon =
            toml::from_str(r#"health_port = "{{ daemons.redis.port }}""#).unwrap();
        let hp = daemon.health_port.unwrap();
        assert!(hp.port.is_none());
        assert_eq!(hp.template.as_deref(), Some("{{ daemons.redis.port }}"));
        assert!(hp.interval.is_none());
        assert!(hp.retries.is_none());
    }

    #[test]
    fn test_health_port_object_form() {
        let daemon: TestDaemon = toml::from_str(
            r#"
health_port = { port = 8443, interval = "10s", retries = 3 }
"#,
        )
        .unwrap();
        let hp = daemon.health_port.unwrap();
        assert_eq!(hp.port, Some(8443));
        assert!(hp.template.is_none());
        assert_eq!(hp.interval, Some(Duration::from_secs(10)));
        assert_eq!(hp.retries, Some(3));
    }

    #[test]
    fn test_health_port_out_of_range_rejected() {
        let err = toml::from_str::<TestDaemon>("health_port = 0").unwrap_err();
        assert!(
            err.to_string()
                .contains("health_port out of range (1-65535)"),
            "unexpected error: {err}"
        );
        let err = toml::from_str::<TestDaemon>("health_port = 70000").unwrap_err();
        assert!(
            err.to_string()
                .contains("health_port out of range (1-65535)"),
            "unexpected error: {err}"
        );
        let err = toml::from_str::<TestDaemon>(
            r#"
health_port = { port = 0 }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("between 1 and 65535"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_port_invalid_interval_rejected() {
        let err = toml::from_str::<TestDaemon>(
            r#"
health_port = { port = 8443, interval = "not-a-duration" }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid interval"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_port_retries_zero_rejected() {
        let err = toml::from_str::<TestDaemon>(
            r#"
health_port = { port = 8443, retries = 0 }
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("health_port retries must be >= 1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_health_port_serialize_roundtrip() {
        let daemon = TestDaemon {
            health_cmd: None,
            health_http: None,
            health_port: Some(HealthPort {
                port: Some(8443),
                template: None,
                interval: Some(Duration::from_secs(10)),
                retries: Some(3),
            }),
        };
        let serialized = toml::to_string(&daemon).unwrap();
        let back: TestDaemon = toml::from_str(&serialized).unwrap();
        assert_eq!(back.health_port, daemon.health_port);

        let shorthand = TestDaemon {
            health_cmd: None,
            health_http: None,
            health_port: Some(HealthPort::new(8443)),
        };
        let serialized = toml::to_string(&shorthand).unwrap();
        assert!(serialized.contains("health_port = 8443"));
    }
}
