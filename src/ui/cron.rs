//! Shared rendering for a cron daemon's schedule timing.
//!
//! `pitchfork status` and the TUI detail pane show the same two lines, so the
//! wording and the relative-time rule live here rather than in each renderer.

use chrono::{DateTime, Local};

use crate::daemon_status::DaemonStatus;
use crate::procs::format_duration;

/// A timestamp with how far it is from `now`, e.g.
/// `2026-09-21 03:00:00 (4h 12m ago)` or `2026-09-22 03:00:00 (in 19h 47m)`.
///
/// A past "next run" is labelled `overdue` rather than `ago`: the watcher has
/// not taken that window yet, which is a different thing from it having run.
pub(crate) fn format_at(at: DateTime<Local>, now: DateTime<Local>, future: bool) -> String {
    let stamp = at.format("%Y-%m-%d %H:%M:%S");
    let delta = (at - now).num_seconds();
    let rel = if delta >= 0 {
        format!("in {}", format_duration(delta as u64))
    } else if future {
        format!("{} overdue", format_duration(delta.unsigned_abs()))
    } else {
        format!("{} ago", format_duration(delta.unsigned_abs()))
    };
    format!("{stamp} ({rel})")
}

/// How the run at `last_cron_run` turned out, as the suffix that follows its
/// timestamp.
///
/// `last_exit_success` describes the daemon's last *finished* run and is not
/// cleared when a new one starts, so while a process is up it still holds the
/// previous run's result. Attaching that to a run still in flight would show
/// a long backup as `failed` for its whole duration, so a live daemon reports
/// no outcome yet. Empty when nothing has finished and nothing is running.
pub(crate) fn run_outcome(status: &DaemonStatus, last_exit_success: Option<bool>) -> &'static str {
    if status.is_running() {
        return " still running";
    }
    match last_exit_success {
        Some(true) => " success",
        Some(false) => " failed",
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 21, h, m, 0).unwrap()
    }

    #[test]
    fn past_time_reads_as_ago() {
        let s = format_at(at(3, 0), at(7, 12), false);
        assert_eq!(s, "2026-09-21 03:00:00 (4h 12m ago)");
    }

    #[test]
    fn future_time_reads_as_in() {
        let s = format_at(at(9, 30), at(7, 0), true);
        assert_eq!(s, "2026-09-21 09:30:00 (in 2h 30m)");
    }

    /// A next run in the past means the watcher owes a window, not that the
    /// daemon ran then.
    #[test]
    fn past_next_run_reads_as_overdue() {
        let s = format_at(at(3, 0), at(5, 0), true);
        assert_eq!(s, "2026-09-21 03:00:00 (2h 0m overdue)");
    }

    #[test]
    fn outcome_is_empty_until_a_run_finishes() {
        assert_eq!(run_outcome(&DaemonStatus::Stopped, None), "");
        assert_eq!(run_outcome(&DaemonStatus::Stopped, Some(true)), " success");
        assert_eq!(run_outcome(&DaemonStatus::Stopped, Some(false)), " failed");
    }

    /// The previous run's result must not be pinned to a run that is still
    /// going: `last_exit_success` survives the start of a new one.
    #[test]
    fn outcome_of_a_live_run_is_not_the_previous_result() {
        assert_eq!(
            run_outcome(&DaemonStatus::Running, Some(false)),
            " still running"
        );
        assert_eq!(
            run_outcome(&DaemonStatus::Running, Some(true)),
            " still running"
        );
    }
}
