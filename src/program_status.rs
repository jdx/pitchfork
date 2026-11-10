//! Program Status Protocol (OSC 7501) reports from daemon output.
//!
//! A daemon can tell its supervisor what it is doing by writing
//! `ESC ] 7501 ; state=working:progress=40:msg=<base64> ESC \` (or ending in
//! BEL). Only the root record is tracked: a report that names an `id` belongs
//! to a child record and is ignored.
//!
//! Spec: <https://www.superlogical.com/rex/docs/build/program-status>

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use std::fmt;

const PREFIX: &str = "\x1b]7501;";
const MAX_SEQUENCE: usize = 4096;
const MAX_KEY: usize = 16;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG_DECODED: usize = 2048;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_TITLE_DECODED: usize = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl ProgramState {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "done" => Self::Done,
            "blocked" => Self::Blocked,
            "error" => Self::Error,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }

    /// `working` and `blocked` describe a live process; `done` and `error`
    /// outlive it.
    pub fn ends_with_process(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }
}

impl fmt::Display for ProgramState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The last root-record status a daemon reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramStatus {
    pub state: ProgramState,
    /// 0 to 100; only kept for `working` and `blocked`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub progress: Option<u8>,
    /// One human-readable line, with control and invisible formatting
    /// characters removed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub msg: Option<String>,
}

impl ProgramStatus {
    /// `working 40% Syncing issues`
    pub fn summary(&self) -> String {
        let mut out = self.state.to_string();
        if let Some(p) = self.progress {
            out.push_str(&format!(" {p}%"));
        }
        if let Some(msg) = &self.msg {
            out.push(' ');
            out.push_str(msg);
        }
        out
    }
}

/// What a report does to the root record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusUpdate {
    Set(ProgramStatus),
    Clear,
}

/// Every valid root-record report in `line`, in order.
pub fn parse_line(line: &str) -> Vec<StatusUpdate> {
    let mut updates = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find(PREFIX) {
        let after = &rest[start + PREFIX.len()..];
        // ST is ESC \ or BEL. A sequence with neither is not a report yet.
        let Some((end, term_len)) = find_terminator(after) else {
            break;
        };
        if PREFIX.len() + end + term_len <= MAX_SEQUENCE
            && let Some(update) = parse_body(&after[..end])
        {
            updates.push(update);
        }
        rest = &after[end + term_len..];
    }
    updates
}

fn find_terminator(s: &str) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    (0..bytes.len()).find_map(|i| match bytes[i] {
        0x07 => Some((i, 1)),
        0x1b if bytes.get(i + 1) == Some(&b'\\') => Some((i, 2)),
        _ => None,
    })
}

fn parse_body(body: &str) -> Option<StatusUpdate> {
    // Feature detection query; nothing to record.
    if body.trim() == "?" {
        return None;
    }
    let mut state = None;
    let mut id_present = false;
    let mut progress = None;
    let mut msg = None;
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty()
            || key.len() > MAX_KEY
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
        {
            continue;
        }
        match key {
            "state" => state = Some(value),
            "id" => id_present = true,
            "progress" => {
                progress = value.parse::<u8>().ok().filter(|p| *p <= 100);
            }
            "msg" => msg = Some(value),
            "title" => {
                // Not shown, but a title that breaks a limit voids the report.
                decode_text(value, MAX_TITLE_ENCODED, MAX_TITLE_DECODED)?;
            }
            _ => {}
        }
    }
    let state_str = state?;
    if state_str == "clear" {
        // `clear` with an id removes a child record, which is not tracked.
        return (!id_present).then_some(StatusUpdate::Clear);
    }
    let state = ProgramState::parse(state_str)?;
    let msg = match msg {
        Some(m) => {
            Some(decode_text(m, MAX_MSG_ENCODED, MAX_MSG_DECODED)?).filter(|m| !m.is_empty())
        }
        None => None,
    };
    if id_present {
        return None;
    }
    let progress =
        progress.filter(|_| matches!(state, ProgramState::Working | ProgramState::Blocked));
    Some(StatusUpdate::Set(ProgramStatus {
        state,
        progress,
        msg,
    }))
}

/// Decode a base64 free-text value. `None` voids the report: a limit broken, bad
/// base64 or UTF-8, or a control character in the text. Invisible formatting
/// characters, such as text direction overrides, are dropped.
fn decode_text(encoded: &str, max_encoded: usize, max_decoded: usize) -> Option<String> {
    if encoded.len() > max_encoded {
        return None;
    }
    let bytes = STANDARD.decode(encoded).ok()?;
    if bytes.len() > max_decoded {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(text.chars().filter(|c| !is_invisible_format(*c)).collect())
}

fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(s: &str) -> String {
        STANDARD.encode(s)
    }

    fn set(state: ProgramState, progress: Option<u8>, msg: Option<&str>) -> StatusUpdate {
        StatusUpdate::Set(ProgramStatus {
            state,
            progress,
            msg: msg.map(str::to_string),
        })
    }

    #[test]
    fn parses_state_progress_and_message() {
        let line = format!(
            "\x1b]7501;state=working:progress=40:msg={}\x1b\\",
            enc("Syncing issues")
        );
        assert_eq!(
            parse_line(&line),
            vec![set(ProgramState::Working, Some(40), Some("Syncing issues"))]
        );
    }

    #[test]
    fn accepts_bel_terminator_and_surrounding_output() {
        let line = "before \x1b]7501;state=done\x07 after";
        assert_eq!(parse_line(line), vec![set(ProgramState::Done, None, None)]);
    }

    #[test]
    fn returns_reports_in_order() {
        let line = "\x1b]7501;state=working\x07\x1b]7501;state=error\x07";
        assert_eq!(
            parse_line(line),
            vec![
                set(ProgramState::Working, None, None),
                set(ProgramState::Error, None, None)
            ]
        );
    }

    #[test]
    fn progress_only_with_working_or_blocked() {
        assert_eq!(
            parse_line("\x1b]7501;state=done:progress=50\x07"),
            vec![set(ProgramState::Done, None, None)]
        );
        assert_eq!(
            parse_line("\x1b]7501;state=blocked:kind=auth:progress=7\x07"),
            vec![set(ProgramState::Blocked, Some(7), None)]
        );
        assert_eq!(
            parse_line("\x1b]7501;state=working:progress=101\x07"),
            vec![set(ProgramState::Working, None, None)]
        );
    }

    #[test]
    fn ignores_unknown_state_and_missing_state() {
        assert!(parse_line("\x1b]7501;state=sleeping\x07").is_empty());
        assert!(parse_line("\x1b]7501;progress=3\x07").is_empty());
    }

    #[test]
    fn ignores_child_records_and_feature_queries() {
        assert!(parse_line("\x1b]7501;state=working:id=build/test\x07").is_empty());
        assert!(parse_line("\x1b]7501;?\x07").is_empty());
        assert!(parse_line("\x1b]7501;state=clear:id=build\x07").is_empty());
    }

    #[test]
    fn clear_without_id_clears() {
        assert_eq!(
            parse_line("\x1b]7501;state=clear\x07"),
            vec![StatusUpdate::Clear]
        );
    }

    #[test]
    fn malformed_pairs_are_skipped_and_last_value_wins() {
        let line = "\x1b]7501;junk:state=idle:Bad=1:state=working\x07";
        assert_eq!(
            parse_line(line),
            vec![set(ProgramState::Working, None, None)]
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let line = "\x1b]7501;state=idle:app=cargo:future=1\x07";
        assert_eq!(parse_line(line), vec![set(ProgramState::Idle, None, None)]);
    }

    #[test]
    fn message_with_control_character_voids_the_report() {
        let line = format!("\x1b]7501;state=working:msg={}\x07", enc("a\x1b[31mb"));
        assert!(parse_line(&line).is_empty());
    }

    #[test]
    fn invisible_formatting_is_stripped() {
        let line = format!("\x1b]7501;state=done:msg={}\x07", enc("ok\u{202E}gnp"));
        assert_eq!(
            parse_line(&line),
            vec![set(ProgramState::Done, None, Some("okgnp"))]
        );
    }

    #[test]
    fn oversized_message_voids_the_report() {
        let line = format!(
            "\x1b]7501;state=working:msg={}\x07",
            enc(&"x".repeat(MAX_MSG_DECODED + 1))
        );
        assert!(parse_line(&line).is_empty());
    }

    #[test]
    fn invalid_base64_voids_the_report() {
        // A byte outside the value set skips only that pair.
        assert_eq!(
            parse_line("\x1b]7501;state=working:msg=@@@\x07"),
            vec![set(ProgramState::Working, None, None)]
        );
        assert!(parse_line("\x1b]7501;state=working:msg=abc\x07").is_empty());
    }

    #[test]
    fn unterminated_sequence_is_not_a_report() {
        assert!(parse_line("\x1b]7501;state=working").is_empty());
    }

    #[test]
    fn summary_joins_parts() {
        let status = ProgramStatus {
            state: ProgramState::Working,
            progress: Some(40),
            msg: Some("Syncing".into()),
        };
        assert_eq!(status.summary(), "working 40% Syncing");
    }
}
