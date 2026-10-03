//! Deprecation warnings that are tied to calendar dates.
//!
//! [`deprecated_at!`] warns once per id from the date a feature is deprecated,
//! and trips a debug assertion once the date it was meant to be removed has
//! passed, so deprecated code cannot linger unnoticed.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// Today's date in UTC.
pub fn today() -> chrono::NaiveDate {
    chrono::Utc::now().date_naive()
}

/// Parse a `YYYY-MM-DD` date; panics on malformed input (a programmer error).
pub fn date(s: &str) -> chrono::NaiveDate {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .unwrap_or_else(|_| panic!("invalid date {s:?} in deprecated_at!, expected YYYY-MM-DD"))
}

/// Ids that have already warned in this process.
pub static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Warn that something is deprecated, once per `$id`.
///
/// ```ignore
/// deprecated_at!("2026-10-03", "2027-10-03", format!("port-fields:{name}"), "use `port` instead.");
/// ```
///
/// Dates are `YYYY-MM-DD`, compared against today's date in UTC.
///
/// - Before `$warn_at` nothing is printed.
/// - From `$warn_at` the message is logged once per process for each `$id`.
/// - From `$remove_at` a debug build panics, as a reminder to delete the
///   deprecated code.
///
/// The remaining arguments are a `format!` message.
#[macro_export]
macro_rules! deprecated_at {
    ($warn_at:expr, $remove_at:expr, $id:expr, $($arg:tt)*) => {{
        let warn_date = $crate::deprecated::date($warn_at);
        let remove_date = $crate::deprecated::date($remove_at);
        let today = $crate::deprecated::today();
        debug_assert!(
            today < remove_date,
            "Deprecated code [{}] should have been removed on {}. Please remove this deprecated functionality.",
            $id, $remove_at
        );
        if today >= warn_date
            && $crate::deprecated::WARNED.lock().unwrap().insert(($id).to_string())
        {
            ::log::warn!(
                "deprecated [{}]: {} This will be removed after {}.",
                $id, format!($($arg)*), $remove_at
            );
        }
    }};
}

#[cfg(test)]
mod tests {
    #[test]
    fn warns_once_per_id() {
        deprecated_at!("2000-01-01", "9999-01-01", "test-once", "old thing.");
        deprecated_at!("2000-01-01", "9999-01-01", "test-once", "old thing.");
        assert!(super::WARNED.lock().unwrap().contains("test-once"));
    }

    #[test]
    fn silent_before_warn_version() {
        deprecated_at!("9998-01-01", "9999-01-01", "test-future", "not yet.");
        assert!(!super::WARNED.lock().unwrap().contains("test-future"));
    }
}
