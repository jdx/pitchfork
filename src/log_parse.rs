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

    let level = extract_level(obj);
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
        // Try to parse the value as a JSON type (number, bool, null);
        // fall back to string.
        let json_val = if value.is_empty() {
            Value::Bool(true)
        } else if let Ok(n) = value.parse::<i64>() {
            Value::Number(n.into())
        } else if let Ok(n) = value.parse::<f64>() {
            serde_json::Number::from_f64(n)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(value.clone()))
        } else if value.eq_ignore_ascii_case("true") {
            Value::Bool(true)
        } else if value.eq_ignore_ascii_case("false") {
            Value::Bool(false)
        } else if value.eq_ignore_ascii_case("null") {
            Value::Null
        } else {
            Value::String(value.clone())
        };
        obj.insert(key.clone(), json_val);
    }

    let value = Value::Object(obj);
    let level = extract_level(value.as_object().unwrap());
    let msg = extract_msg(value.as_object().unwrap());
    let logger = extract_logger(value.as_object().unwrap());
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
/// Returns `None` if the line doesn't look like logfmt (no `=` found, or
/// parsing yields zero pairs).
fn parse_logfmt_pairs(line: &str) -> Option<Vec<(String, String)>> {
    let bytes = line.as_bytes();
    let mut pairs = Vec::new();
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
                pairs.push((key.to_string(), value));
            } else {
                // Unquoted value: read until whitespace or end.
                let val_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                pairs.push((key.to_string(), line[val_start..i].to_string()));
            }
        } else {
            // Bare key (no '='): treat as boolean true.
            pairs.push((key.to_string(), String::new()));
        }
    }

    if pairs.is_empty() {
        return None;
    }
    // Require at least one pair with '=' to distinguish from plain text.
    if !line.contains('=') {
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
fn extract_level(obj: &Map<String, Value>) -> Option<String> {
    for key in &["level", "severity", "lvl", "PRIORITY", "@level"] {
        if let Some(val) = obj.get(*key) {
            if let Some(level) = normalize_level_value(val) {
                return Some(level);
            }
        }
    }
    None
}

/// Normalize a level value to one of: error, warn, info, debug, trace.
///
/// String matching is case-insensitive. Integer values follow pino
/// (10-60) and syslog/RFC 5424 (0-7) conventions.
fn normalize_level_value(val: &Value) -> Option<String> {
    match val {
        Value::String(s) => normalize_level_str(s),
        Value::Number(n) => {
            let n = n.as_i64()?;
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

/// Try to extract the human-readable message from common field names.
fn extract_msg(obj: &Map<String, Value>) -> Option<String> {
    for key in &["msg", "message", "event", "@message"] {
        if let Some(Value::String(s)) = obj.get(*key) {
            return Some(s.clone());
        }
    }
    None
}

/// Try to extract the logger name from common field names.
fn extract_logger(obj: &Map<String, Value>) -> Option<String> {
    for key in &["logger", "name", "component", "module"] {
        if let Some(Value::String(s)) = obj.get(*key) {
            return Some(s.clone());
        }
    }
    None
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
    fn test_json_nested_not_extracted_as_msg() {
        // Only top-level fields are extracted; nested objects stay in fields_json.
        let line = r#"{"level":"info","fields":{"message":"nested"}}"#;
        let parsed = parse(line, "json");
        assert_eq!(parsed.level.as_deref(), Some("info"));
        assert_eq!(parsed.msg, None); // top-level has no msg/message/event
    }
}
