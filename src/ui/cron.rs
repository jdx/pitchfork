//! Shared rendering for a cron daemon's schedule timing.
//!
//! `pitchfork status` and the TUI detail pane show the same two lines, so the
//! wording and the relative-time rule live here rather than in each renderer.

use chrono::{DateTime, Local};

use crate::procs::format_duration;

/// A timestamp with how far it is from `now`, e.g.
/// `2026-09-21 03:00:00 (4h 12m ago)` or `2026-09-22 03:00:00 (in 19h 47m)`.
///
/// A past "next run" is labelled `overdue` rather than `ago`: the watcher has
/// not taken that window yet, which is a different thing from it having run.
pub(crate) fn format_at(at: DateTime<Local>, now: DateTime<Local>, future: bool) -> String {
    let stamp = at.format("%Y-%m-%d %H:%M:%S");
    let delta = (at - now).num_seconds();
    let rel = if future {
        if delta >= 0 {
            format!("in {}", format_duration(delta as u64))
        } else {
            format!("{} overdue", format_duration(delta.unsigned_abs()))
        }
    } else {
        // A recorded run has already happened, so the same second reads as
        // `0s ago` rather than `in 0s`. `min(0)` also absorbs a timestamp that
        // reads as slightly ahead of now, which a clock adjustment between the
        // write and the read can produce.
        format!("{} ago", format_duration(delta.min(0).unsigned_abs()))
    };
    format!("{stamp} ({rel})")
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

    /// A run recorded this second is still a past event.
    #[test]
    fn a_run_in_the_same_second_reads_as_ago() {
        assert_eq!(
            format_at(at(3, 0), at(3, 0), false),
            "2026-09-21 03:00:00 (0s ago)"
        );
    }

    /// A timestamp that reads as slightly ahead of now -- a clock adjustment
    /// between the write and the read -- is still a past event.
    #[test]
    fn a_past_event_never_reads_as_the_future() {
        assert_eq!(
            format_at(at(3, 1), at(3, 0), false),
            "2026-09-21 03:01:00 (0s ago)"
        );
    }

    /// A next run in the past means the watcher owes a window, not that the
    /// daemon ran then.
    #[test]
    fn past_next_run_reads_as_overdue() {
        let s = format_at(at(3, 0), at(5, 0), true);
        assert_eq!(s, "2026-09-21 03:00:00 (2h 0m overdue)");
    }
}
