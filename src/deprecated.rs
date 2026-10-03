//! Deprecation warnings that are tied to pitchfork versions.
//!
//! [`deprecated_at!`] warns once per id from the version that deprecates a
//! feature, and trips a debug assertion once the version that was meant to
//! remove it has been reached, so deprecated code cannot linger unnoticed.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// The running pitchfork version.
pub static VERSION: LazyLock<semver::Version> = LazyLock::new(|| {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver")
});

/// Ids that have already warned in this process.
pub static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Warn that something is deprecated, once per `$id`.
///
/// ```ignore
/// deprecated_at!("2.30.0", "3.0.0", format!("port-fields:{name}"), "use `port` instead.");
/// ```
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
        let warn_version = ::semver::Version::parse($warn_at)
            .expect("invalid warn_at version in deprecated_at!");
        let remove_version = ::semver::Version::parse($remove_at)
            .expect("invalid remove_at version in deprecated_at!");
        debug_assert!(
            *$crate::deprecated::VERSION < remove_version,
            "Deprecated code [{}] should have been removed in version {}. Please remove this deprecated functionality.",
            $id, $remove_at
        );
        if *$crate::deprecated::VERSION >= warn_version
            && $crate::deprecated::WARNED.lock().unwrap().insert(($id).to_string())
        {
            ::log::warn!(
                "deprecated [{}]: {} This will be removed in pitchfork {}.",
                $id, format!($($arg)*), $remove_at
            );
        }
    }};
}

#[cfg(test)]
mod tests {
    #[test]
    fn warns_once_per_id() {
        deprecated_at!("0.1.0", "999.0.0", "test-once", "old thing.");
        deprecated_at!("0.1.0", "999.0.0", "test-once", "old thing.");
        assert!(super::WARNED.lock().unwrap().contains("test-once"));
    }

    #[test]
    fn silent_before_warn_version() {
        deprecated_at!("998.0.0", "999.0.0", "test-future", "not yet.");
        assert!(!super::WARNED.lock().unwrap().contains("test-future"));
    }
}
