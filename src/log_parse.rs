//! Structured log line parsing.
//!
//! Parses daemon stdout/stderr lines into structured fields (level, msg,
//! logger, fields_json) based on the configured `log_format`. Supports JSON
//! and logfmt formats.

use serde_json::{Map, Value};

/// Result of parsing a single log line.
///
/// `message` always holds the original raw line text. The structured fields
/// are `None` when the line could not be parsed (e.g. plain text or parse
/// failure).
#[derive(Debug, Clone, Default)]
pub struct ParsedLog {
    /// The original raw line text, always preserved.
    pub message: String,
    /// Normalized log level: `error` | `warn` | `info` | `debug` | `trace`.
    pub level: Option<String>,
    /// Extracted human-readable message (from `msg`/`message`/`event`/...).
    pub msg: Option<String>,
    /// Logger name (from `logger`/`name`/`component`/...).
    pub logger: Option<String>,
    /// The full parsed JSON object as a string, for `json_extract` queries.
    /// `None` for plain-text or logfmt lines (logfmt fields are also stored
    /// here as a JSON object string).
    pub fields_json: Option<String>,
}

impl ParsedLog {
    /// Create a plain-text ParsedLog with no structured fields.
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Default::default()
        }
    }
}

/// Maximum line length for structured parsing. Lines exceeding this are
/// stored as plain text without field extraction, protecting against
/// pathological inputs that would cause excessive memory or CPU use.
const MAX_PARSE_LINE_LEN: usize = 65536;

/// Parse a log line according to the given format string.
///
/// Format values: `"json"`, `"logfmt"`, `"text"` (or any other
/// value is treated as text). Parse failures fall back to plain text.
pub fn parse(line: &str, format: &str) -> ParsedLog {
    if line.len() > MAX_PARSE_LINE_LEN {
        return ParsedLog::plain(line);
    }
    match format {
        "json" => parse_json(line).unwrap_or_else(|| ParsedLog::plain(line)),
        "logfmt" => parse_logfmt(line).unwrap_or_else(|| ParsedLog::plain(line)),
        _ => ParsedLog::plain(line),
    }
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

fn parse_json(line: &str) -> Option<ParsedLog> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let obj = value.as_object()?;

    let level = extract_level(obj, false);
    let msg = extract_msg(obj);
    let logger = extract_logger(obj);

    // Re-serialize as compact JSON for storage. Using the original object
    // (not a filtered subset) so all fields are available for json_extract.
    let fields_json = serde_json::to_string(&value).ok()?;

    Some(ParsedLog {
        message: line.to_string(),
        level,
        msg,
        logger,
        fields_json: Some(fields_json),
    })
}

// ---------------------------------------------------------------------------
// logfmt parsing
// ---------------------------------------------------------------------------

fn parse_logfmt(line: &str) -> Option<ParsedLog> {
    let pairs = parse_logfmt_pairs(line)?;

    // Build a JSON object from the key-value pairs.
    let mut obj = Map::new();
    for (key, value) in &pairs {
        let json_val = match value {
            // Bare key (no '=') → boolean true.
            LogfmtValue::Bare => Value::Bool(true),
            // Quoting is the one way logfmt says a value is text.
            LogfmtValue::Quoted(value) => Value::String(value.clone()),
            LogfmtValue::Unquoted(value) => unquoted_logfmt_value(value),
        };
        obj.insert(key.clone(), json_val);
    }

    // Logfmt has no numbers of its own, so `level="30"` is pino's info.
    let level = extract_level(&obj, true);
    let msg = extract_msg(&obj);
    let logger = extract_logger(&obj);
    let value = Value::Object(obj);
    let fields_json = serde_json::to_string(&value).ok()?;

    Some(ParsedLog {
        message: line.to_string(),
        level,
        msg,
        logger,
        fields_json: Some(fields_json),
    })
}

/// Parse a logfmt line into (key, value) pairs.
///
/// Grammar (simplified from kr/logfmt):
/// ```text
/// pair = key '=' value | key '=' | key
/// key  = ident
/// value = ident | '"...' '"'
/// ```
///
/// A logfmt value as written. Logfmt has no types; quoting is the only
/// thing a writer can say about a value.
enum LogfmtValue {
    /// A key without `=`, distinct from an explicit empty value (`key=""`
    /// or `key=`).
    Bare,
    Quoted(String),
    Unquoted(String),
}

/// The JSON value for an unquoted logfmt value: a number, boolean or null
/// when that is exactly how the text spells it, so nothing is lost
/// converting it; otherwise the text itself. `007`, `1.10`, `TRUE` and `1e3`
/// therefore stay strings, as they would print differently as JSON.
fn unquoted_logfmt_value(value: &str) -> Value {
    let typed = match value {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        "null" => Some(Value::Null),
        _ => value
            .parse::<i64>()
            .ok()
            .map(|n| Value::Number(n.into()))
            .or_else(|| {
                value
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
            }),
    };
    match typed {
        Some(typed) if json_spelling_is(&typed, value) => typed,
        _ => Value::String(value.to_string()),
    }
}

/// Whether `typed` is written as JSON exactly as `text`, checked in a stack
/// buffer: this runs for every numeric field of every logfmt line.
fn json_spelling_is(typed: &Value, text: &str) -> bool {
    // Longer than any number, boolean or null serde_json writes.
    let mut buf = [0u8; 40];
    let mut cursor = std::io::Cursor::new(&mut buf[..]);
    if serde_json::to_writer(&mut cursor, typed).is_err() {
        return false;
    }
    let len = cursor.position() as usize;
    &buf[..len] == text.as_bytes()
}

/// Returns `None` if the line doesn't look like logfmt (no `=` found, or
/// parsing yields zero pairs).
fn parse_logfmt_pairs(line: &str) -> Option<Vec<(String, LogfmtValue)>> {
    let bytes = line.as_bytes();
    let mut pairs = Vec::new();
    let mut bare_key_count = 0;
    let mut i = 0;

    while i < bytes.len() {
        // Skip whitespace and garbage between pairs.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        // Parse key: read until '=', whitespace, or end.
        let key_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let key = &line[key_start..i];
        if key.is_empty() {
            // Skip stray garbage.
            i += 1;
            continue;
        }
        // Reject keys that aren't valid logfmt identifiers. Real logfmt keys
        // are alphanumeric with '_', '.', '-', '@'. Tokens like '2026/07/23' or
        // '22:02:39' (from Go's standard log format) are not valid keys.
        // '@' is allowed for pino/syslog style keys like @level, @message.
        if !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-' || b == b'@')
        {
            // Skip the entire invalid token. If followed by '=value',
            // consume that too so it isn't parsed as a separate pair.
            while i < bytes.len() && bytes[i] == b'=' {
                i += 1;
                if i < bytes.len() && bytes[i] == b'"' {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' && i + 1 < bytes.len() {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                } else {
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                }
            }
            continue;
        }

        // Check for '='.
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1; // consume '='

            // Parse value.
            if i < bytes.len() && bytes[i] == b'"' {
                // Quoted value: read until closing '"', handling escapes.
                i += 1; // skip opening quote
                let val_start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                let value = unescape_logfmt_value(&line[val_start..i]);
                if i < bytes.len() {
                    i += 1; // skip closing quote
                }
                pairs.push((key.to_string(), LogfmtValue::Quoted(value)));
            } else {
                // Unquoted value: read until whitespace or end.
                let val_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                pairs.push((
                    key.to_string(),
                    LogfmtValue::Unquoted(line[val_start..i].to_string()),
                ));
            }
        } else {
            // Bare key (no '='): treat as boolean true.
            pairs.push((key.to_string(), LogfmtValue::Bare));
            bare_key_count += 1;
        }
    }

    if pairs.is_empty() {
        return None;
    }
    // Require at least one pair with '=' to distinguish from plain text.
    if !line.contains('=') {
        return None;
    }
    // Reject lines that are mostly bare keys — they are likely plain text
    // with a trailing key=value, not real logfmt. A bare key is one without
    // '='; explicit `key=""` or `key=` counts as a proper pair.
    if bare_key_count * 2 > pairs.len() {
        return None;
    }
    Some(pairs)
}

fn unescape_logfmt_value(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                result.push(next);
            }
        } else {
            result.push(c);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Field extraction with alias tables (inspired by pamburus/hl)
// ---------------------------------------------------------------------------

/// Try to extract a normalized level from common field names.
///
/// Handles both string values (case-insensitive) and integer values
/// (pino/syslog style). See `normalize_level_value` for the mapping.
/// `numeric_text` also reads a string of digits as an integer level, for
/// formats like logfmt whose values are all text.
fn extract_level(obj: &Map<String, Value>, numeric_text: bool) -> Option<String> {
    for key in &["level", "severity", "lvl", "PRIORITY", "@level"] {
        if let Some(val) = obj.get(*key)
            && let Some(level) = normalize_level_value(val, numeric_text)
        {
            return Some(level);
        }
    }
    None
}

/// Normalize a level value to one of: error, warn, info, debug, trace.
///
/// String matching is case-insensitive. Integer values follow pino
/// (10-60) and syslog/RFC 5424 (0-7) conventions.
fn normalize_level_value(val: &Value, numeric_text: bool) -> Option<String> {
    match val {
        Value::String(s) => normalize_level_str(s).or_else(|| {
            numeric_text
                .then(|| s.parse::<i64>().ok())
                .flatten()
                .and_then(normalize_level_number)
        }),
        Value::Number(n) => normalize_level_number(n.as_i64()?),
        _ => None,
    }
}

/// Normalize an integer level, following pino (10-60) and syslog/RFC 5424
/// (0-7).
fn normalize_level_number(n: i64) -> Option<String> {
    // pino: 10=trace, 20=debug, 30=info, 40=warn, 50=error, 60=fatal
    match n {
        50 | 60 => Some("error".into()),
        40 => Some("warn".into()),
        30 => Some("info".into()),
        20 => Some("debug".into()),
        10 => Some("trace".into()),
        // syslog/RFC 5424: 0=emerg..7=debug
        0..=3 => Some("error".into()),
        4 | 5 => Some("warn".into()),
        6 => Some("info".into()),
        7 => Some("debug".into()),
        _ => None,
    }
}

pub fn normalize_level_str(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "error" | "err" | "fatal" | "critical" | "panic" | "alert" | "emerg" => {
            Some("error".into())
        }
        "warn" | "warning" | "wrn" => Some("warn".into()),
        "info" | "inf" | "information" | "notice" => Some("info".into()),
        "debug" | "dbg" => Some("debug".into()),
        "trace" | "trc" => Some("trace".into()),
        _ => None,
    }
}

fn extract_first_string(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(s)) = obj.get(*key) {
            return Some(s.clone());
        }
    }
    None
}

/// Try to extract the human-readable message from common field names.
fn extract_msg(obj: &Map<String, Value>) -> Option<String> {
    extract_first_string(obj, &["msg", "message", "event", "@message"])
}

/// Try to extract the logger name from common field names.
fn extract_logger(obj: &Map<String, Value>) -> Option<String> {
    extract_first_string(obj, &["logger", "name", "component", "module"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_parse() {
        let line = r#"{"level":"info","msg":"server started","port":8080}"#;
        let parsed = parse(line, "json");
        assert_eq!(parsed.level.as_deref(), Some("info"));
        assert_eq!(parsed.msg.as_deref(), Some("server started"));
        assert!(parsed.fields_json.is_some());
    }

    #[test]
    fn test_json_level_normalization() {
        let line = r#"{"level":"FATAL","msg":"crash"}"#;
        let parsed = parse(line, "json");
        assert_eq!(parsed.level.as_deref(), Some("error"));
    }

    #[test]
    fn test_json_pino_integer_level() {
        let line = r#"{"level":50,"msg":"error occurred"}"#;
        let parsed = parse(line, "json");
        assert_eq!(parsed.level.as_deref(), Some("error"));
    }

    #[test]
    fn test_json_syslog_priority() {
        let line = r#"{"PRIORITY":3,"msg":"system error"}"#;
        let parsed = parse(line, "json");
        assert_eq!(parsed.level.as_deref(), Some("error"));
    }

    #[test]
    fn test_json_msg_aliases() {
        // structlog uses "event"
        let line = r#"{"event":"hello","level":"info"}"#;
        let parsed = parse(line, "json");
        assert_eq!(parsed.msg.as_deref(), Some("hello"));
    }

    #[test]
    fn test_logfmt_parse() {
        let line = r#"level=info msg="server started" port=8080"#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.level.as_deref(), Some("info"));
        assert_eq!(parsed.msg.as_deref(), Some("server started"));
        assert!(parsed.fields_json.is_some());
    }

    fn logfmt_fields(line: &str) -> Value {
        let parsed = parse(line, "logfmt");
        serde_json::from_str(parsed.fields_json.as_deref().unwrap()).unwrap()
    }

    #[test]
    fn test_logfmt_types_values_only_when_nothing_is_lost() {
        let fields = logfmt_fields(
            "level=info status=500 took=0.25 neg=-3 ok=true gone=null id=007 version=1.10 \
             big=12345678901234567890 exp=1e3 flag=TRUE",
        );
        assert_eq!(fields["status"], serde_json::json!(500));
        assert_eq!(fields["took"], serde_json::json!(0.25));
        assert_eq!(fields["neg"], serde_json::json!(-3));
        assert_eq!(fields["ok"], Value::Bool(true));
        assert_eq!(fields["gone"], Value::Null);
        for (key, text) in [
            ("id", "007"),
            ("version", "1.10"),
            ("big", "12345678901234567890"),
            ("exp", "1e3"),
            ("flag", "TRUE"),
        ] {
            assert_eq!(fields[key], Value::String(text.into()), "{key}");
        }
    }

    #[test]
    fn test_logfmt_quoted_values_stay_strings() {
        let fields = logfmt_fields(r#"level=info code="007" count="42" ok="true""#);
        assert_eq!(fields["code"], Value::String("007".into()));
        assert_eq!(fields["count"], Value::String("42".into()));
        assert_eq!(fields["ok"], Value::String("true".into()));
    }

    /// A numeric level keeps its text in the fields but is still mapped to a
    /// severity, quoted or not, as it was when it was stored as a number.
    #[test]
    fn test_logfmt_numeric_level_text_is_normalized() {
        for (line, level) in [
            (r#"level="30" msg=hi"#, "info"),
            ("level=50 msg=hi", "error"),
            (r#"PRIORITY="3" msg=hi"#, "error"),
            ("level=03 msg=hi", "error"),
        ] {
            assert_eq!(
                parse(line, "logfmt").level.as_deref(),
                Some(level),
                "{line}"
            );
        }
        assert_eq!(
            logfmt_fields(r#"level="30" msg=hi"#)["level"],
            Value::String("30".into())
        );
    }

    /// JSON keeps its own types, so a level written as a string is not read
    /// as a number.
    #[test]
    fn test_json_numeric_level_string_is_not_normalized() {
        assert_eq!(parse(r#"{"level":"30","msg":"hi"}"#, "json").level, None);
    }

    #[test]
    fn test_logfmt_bare_key() {
        let line = r#"level=debug ready msg="ok""#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.level.as_deref(), Some("debug"));
        assert_eq!(parsed.msg.as_deref(), Some("ok"));
        // "ready" is a bare key → true
        let fields: Value = serde_json::from_str(parsed.fields_json.as_deref().unwrap()).unwrap();
        assert_eq!(fields["ready"], Value::Bool(true));
    }

    #[test]
    fn test_logfmt_quoted_value_with_spaces() {
        let line = r#"level=error msg="connection refused: timeout""#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.msg.as_deref(), Some("connection refused: timeout"));
    }

    #[test]
    fn test_text_format() {
        let line = r#"{"level":"info"}"#;
        let parsed = parse(line, "text");
        assert!(parsed.level.is_none());
        assert!(parsed.fields_json.is_none());
        assert_eq!(parsed.message, line);
    }

    #[test]
    fn test_json_parse_failure_falls_back() {
        let line = "{not valid json";
        let parsed = parse(line, "json");
        assert!(parsed.level.is_none());
        assert!(parsed.fields_json.is_none());
        assert_eq!(parsed.message, line);
    }

    #[test]
    fn test_logfmt_logger_extraction() {
        let line = r#"level=info msg="hi" logger=myapp"#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.logger.as_deref(), Some("myapp"));
    }

    #[test]
    fn test_logfmt_multiple_bare_keys_still_parsed() {
        // 2 bare keys + 2 key=value: proper=2, 2*2=4 not < 4, so accepted.
        let line = r#"level=debug ready enabled msg="ok""#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.level.as_deref(), Some("debug"));
        assert_eq!(parsed.msg.as_deref(), Some("ok"));
        let fields: Value = serde_json::from_str(parsed.fields_json.as_deref().unwrap()).unwrap();
        assert_eq!(fields["ready"], Value::Bool(true));
        assert_eq!(fields["enabled"], Value::Bool(true));
    }

    #[test]
    fn test_logfmt_rejects_go_standard_log() {
        // Go standard log format with a trailing key=value should not be
        // misparse as logfmt.
        let line = "2026/07/23 22:02:39 INFO acquired instance lock path=/foo/bar";
        let parsed = parse(line, "logfmt");
        // Should fall back to plain text — no structured fields.
        assert!(parsed.level.is_none());
        assert!(parsed.fields_json.is_none());
        assert_eq!(parsed.message, line);
    }

    #[test]
    fn test_logfmt_explicit_empty_value() {
        // `key=""` is an explicit empty value, not a bare key.
        // Should not be rejected by the bare-key ratio check.
        let line = r#"level=debug msg="" extra="""#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.level.as_deref(), Some("debug"));
        assert_eq!(parsed.msg.as_deref(), Some(""));
        let fields: Value = serde_json::from_str(parsed.fields_json.as_deref().unwrap()).unwrap();
        assert_eq!(fields["msg"], Value::String("".into()));
        assert_eq!(fields["extra"], Value::String("".into()));
    }

    #[test]
    fn test_logfmt_hyphenated_key() {
        // Hyphens are valid in logfmt keys (e.g. request-id).
        let line = r#"level=info msg="ok" request-id=abc123"#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.level.as_deref(), Some("info"));
        let fields: Value = serde_json::from_str(parsed.fields_json.as_deref().unwrap()).unwrap();
        assert_eq!(fields["request-id"], Value::String("abc123".into()));
    }

    #[test]
    fn test_logfmt_at_prefixed_key() {
        // pino/syslog style keys use @ prefix (e.g. @level, @message).
        let line = r#"@level=info @message="server started" port=8080"#;
        let parsed = parse(line, "logfmt");
        assert_eq!(parsed.level.as_deref(), Some("info"));
        assert_eq!(parsed.msg.as_deref(), Some("server started"));
        let fields: Value = serde_json::from_str(parsed.fields_json.as_deref().unwrap()).unwrap();
        assert_eq!(fields["port"], Value::Number(8080.into()));
    }

    #[test]
    fn test_json_nested_not_extracted_as_msg() {
        // Only top-level fields are extracted; nested objects stay in fields_json.
        let line = r#"{"level":"info","fields":{"message":"nested"}}"#;
        let parsed = parse(line, "json");
        assert_eq!(parsed.level.as_deref(), Some("info"));
        assert_eq!(parsed.msg, None); // top-level has no msg/message/event
    }
}
