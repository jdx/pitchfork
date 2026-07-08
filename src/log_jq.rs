//! jq-style log filtering via [jaq](https://github.com/01mf02/jaq).
//!
//! Compiles a jq expression and applies it to each log entry (serialized as
//! a JSON object). Entries for which the filter produces at least one truthy
//! output are retained.

use crate::log_store::LogEntry;
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{self, Ctx, Vars, data, unwrap_valr};
use jaq_json::{self, Val, read::parse_single, write};

/// A compiled jq filter that can be applied to log entries.
pub struct JqFilter {
    filter: jaq_core::Filter<data::JustLut<Val>>,
}

impl JqFilter {
    /// Compile a jq expression.
    pub fn new(expr: &str) -> miette::Result<Self> {
        let program = File {
            code: expr,
            path: (),
        };
        let defs = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs());
        let funs = jaq_core::funs()
            .chain(jaq_std::funs())
            .chain(jaq_json::funs());
        let loader = Loader::new(defs);
        let arena = Arena::default();
        let modules = loader
            .load(&arena, program)
            .map_err(|e| miette::miette!("jq parse error: {e:?}"))?;
        let filter = jaq_core::Compiler::default()
            .with_funs(funs)
            .compile(modules)
            .map_err(|e| miette::miette!("jq compile error: {e:?}"))?;
        Ok(Self { filter })
    }

    /// Returns true if the filter produces at least one truthy output for
    /// this entry.
    ///
    /// The entry is serialized to a JSON object with all available fields
    /// (timestamp, daemon_id, message, level, msg, logger, fields).
    fn matches(&self, entry: &LogEntry) -> bool {
        let json = serialize_entry(entry);
        let input = match parse_single(json.as_bytes()) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let ctx = Ctx::<data::JustLut<Val>>::new(&self.filter.lut, Vars::new([]));
        let mut outputs = self.filter.id.run((ctx, input)).map(unwrap_valr);
        outputs.any(|result| result.is_ok_and(|val| is_truthy(&val)))
    }

    /// Filter a list of entries, retaining only those that match.
    pub fn filter(&self, entries: Vec<LogEntry>) -> Vec<LogEntry> {
        entries.into_iter().filter(|e| self.matches(e)).collect()
    }
}

/// Serialize a LogEntry to a JSON string for jq input.
///
/// The `fields` sub-object is included when available, so users can write
/// filters like `.fields.request_id == "abc"` or `.level == "error"`.
fn serialize_entry(entry: &LogEntry) -> String {
    let fields = entry
        .fields_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    let obj = serde_json::json!({
        "timestamp": entry.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
        "daemon_id": entry.daemon_id,
        "message": entry.message,
        "level": entry.level,
        "msg": entry.msg,
        "logger": entry.logger,
        "fields": fields,
    });
    obj.to_string()
}

/// Determine if a jaq Val is truthy (not false and not null).
fn is_truthy(val: &Val) -> bool {
    !matches!(val, Val::Bool(false) | Val::Null)
}

/// Format a Val as a compact JSON string (for potential future use in
/// `--jq` output mode).
#[allow(dead_code)]
pub fn val_to_json(val: &Val) -> String {
    let mut buf = Vec::new();
    let pp = write::Pp::default();
    let _ = write::write(&mut buf, &pp, 0, val);
    String::from_utf8_lossy(&buf).to_string()
}
