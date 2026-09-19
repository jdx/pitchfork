//! Per-daemon proxy activity: what the proxy is carrying for each daemon right
//! now, and when it last carried anything.
//!
//! Every unit of work the proxy does for a daemon holds an [`ActivityGuard`]
//! for as long as it lasts:
//!
//! - an HTTP request, until its response body has been sent in full, so a
//!   streamed or server-sent-events response counts for as long as it streams;
//! - an upgraded connection (WebSocket), until either side closes it;
//! - a TLS passthrough connection, until either side closes it;
//! - an auto-start, for the daemon and every dependency it brings up.
//!
//! An idle keep-alive connection holds nothing: it is the requests on it that
//! count. Name resolution is not activity either, since a browser resolves
//! names speculatively.
//!
//! The same lock that counts activity also records a daemon being stopped for
//! inactivity, so the two cannot interleave: a stop is only claimed while
//! nothing is in flight, and nothing new begins while a stop is claimed.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::daemon_id::DaemonId;

/// The process-wide tracker the proxy and the supervisor share.
pub(crate) static ACTIVITY: once_cell::sync::Lazy<ActivityTracker> =
    once_cell::sync::Lazy::new(ActivityTracker::default);

#[derive(Debug)]
struct Entry {
    /// Units of work under way.
    in_flight: usize,
    /// When the last unit of work began or ended, or when the daemon was first
    /// looked at if nothing has happened since.
    last_activity: Instant,
    /// A stop for inactivity has been claimed and not yet released.
    idle_stopping: bool,
}

impl Entry {
    fn new(now: Instant) -> Self {
        Self {
            in_flight: 0,
            last_activity: now,
            idle_stopping: false,
        }
    }
}

/// Activity for every daemon the proxy has done anything for.
#[derive(Debug, Default)]
pub(crate) struct ActivityTracker {
    entries: Mutex<HashMap<DaemonId, Entry>>,
}

/// How a daemon's activity looks from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActivitySnapshot {
    pub in_flight: usize,
    pub idle_for: Duration,
    pub idle_stopping: bool,
}

impl ActivityTracker {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<DaemonId, Entry>> {
        // Bookkeeping only; a panic elsewhere must not wedge the proxy.
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Begin a unit of work for `id`, which lasts until the guard is dropped.
    ///
    /// Returns `None` while the daemon is being stopped for inactivity: the
    /// caller should treat it as not running yet and try again once the stop
    /// has finished, rather than forward to a daemon that is going away.
    pub(crate) fn begin(&'static self, id: &DaemonId) -> Option<ActivityGuard> {
        let now = Instant::now();
        let mut entries = self.lock();
        let entry = entries.entry(id.clone()).or_insert_with(|| Entry::new(now));
        if entry.idle_stopping {
            return None;
        }
        entry.in_flight += 1;
        entry.last_activity = now;
        Some(ActivityGuard {
            tracker: self,
            id: id.clone(),
        })
    }

    /// Begin a unit of work for each of `ids`, or for none of them if any is
    /// being stopped for inactivity.
    pub(crate) fn begin_all(&'static self, ids: &[DaemonId]) -> Option<Vec<ActivityGuard>> {
        let mut guards = Vec::with_capacity(ids.len());
        for id in ids {
            // Dropping the guards taken so far releases them.
            guards.push(self.begin(id)?);
        }
        Some(guards)
    }

    fn end(&self, id: &DaemonId) {
        let mut entries = self.lock();
        if let Some(entry) = entries.get_mut(id) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
            entry.last_activity = Instant::now();
        }
    }

    /// Current activity for `id`.
    ///
    /// A daemon the proxy has never seen is recorded as of now, so a daemon
    /// adopted from a previous supervisor gets a full grace period rather than
    /// being judged idle on the first look.
    pub(crate) fn snapshot(&self, id: &DaemonId) -> ActivitySnapshot {
        let now = Instant::now();
        let mut entries = self.lock();
        let entry = entries.entry(id.clone()).or_insert_with(|| Entry::new(now));
        ActivitySnapshot {
            in_flight: entry.in_flight,
            idle_for: now.saturating_duration_since(entry.last_activity),
            idle_stopping: entry.idle_stopping,
        }
    }

    /// Claim `id` for a stop, provided nothing is in flight and nothing has
    /// happened for at least `grace`.
    ///
    /// While the claim is held, [`Self::begin`] refuses new work for the
    /// daemon. Release it with [`Self::release_idle_stop`] whether or not the
    /// stop went ahead.
    pub(crate) fn claim_idle_stop(&self, id: &DaemonId, grace: Duration) -> bool {
        let now = Instant::now();
        let mut entries = self.lock();
        let entry = entries.entry(id.clone()).or_insert_with(|| Entry::new(now));
        if entry.idle_stopping
            || entry.in_flight > 0
            || now.saturating_duration_since(entry.last_activity) < grace
        {
            return false;
        }
        entry.idle_stopping = true;
        true
    }

    /// Release a claim taken by [`Self::claim_idle_stop`].
    ///
    /// The idle clock restarts, so a stop that was called off does not leave
    /// the daemon immediately eligible again.
    pub(crate) fn release_idle_stop(&self, id: &DaemonId) {
        let mut entries = self.lock();
        if let Some(entry) = entries.get_mut(id) {
            entry.idle_stopping = false;
            entry.last_activity = Instant::now();
        }
    }

    /// Whether a stop for inactivity is under way for `id`.
    pub(crate) fn is_idle_stopping(&self, id: &DaemonId) -> bool {
        self.lock().get(id).is_some_and(|e| e.idle_stopping)
    }
}

/// One unit of proxy work for a daemon. Dropping it ends the work.
#[must_use = "activity ends as soon as the guard is dropped"]
pub(crate) struct ActivityGuard {
    tracker: &'static ActivityTracker,
    id: DaemonId,
}

impl std::fmt::Debug for ActivityGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityGuard")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.tracker.end(&self.id);
    }
}

/// A response body that holds an [`ActivityGuard`] until it has been sent in
/// full or abandoned, so a long download or an event stream counts as
/// activity for as long as it lasts.
pub(crate) struct GuardedBody<B> {
    inner: B,
    _guard: Option<ActivityGuard>,
}

impl<B> GuardedBody<B> {
    pub(crate) fn new(inner: B, guard: Option<ActivityGuard>) -> Self {
        Self {
            inner,
            _guard: guard,
        }
    }
}

impl<B> hyper::body::Body for GuardedBody<B>
where
    B: hyper::body::Body + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        std::pin::Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> &'static ActivityTracker {
        Box::leak(Box::default())
    }

    fn id(name: &str) -> DaemonId {
        DaemonId::new("proj", name)
    }

    #[test]
    fn in_flight_work_blocks_an_idle_claim() {
        let t = tracker();
        let guard = t.begin(&id("api")).unwrap();
        assert!(!t.claim_idle_stop(&id("api"), Duration::ZERO));
        drop(guard);
        assert!(t.claim_idle_stop(&id("api"), Duration::ZERO));
    }

    #[test]
    fn recent_activity_blocks_an_idle_claim_until_the_grace_passes() {
        let t = tracker();
        drop(t.begin(&id("api")).unwrap());
        assert!(!t.claim_idle_stop(&id("api"), Duration::from_secs(60)));
        assert!(t.claim_idle_stop(&id("api"), Duration::ZERO));
    }

    #[test]
    fn a_claimed_stop_refuses_new_work_until_released() {
        let t = tracker();
        assert!(t.claim_idle_stop(&id("api"), Duration::ZERO));
        assert!(t.is_idle_stopping(&id("api")));
        assert!(t.begin(&id("api")).is_none());
        // A second claim for the same daemon is refused too.
        assert!(!t.claim_idle_stop(&id("api"), Duration::ZERO));
        t.release_idle_stop(&id("api"));
        assert!(!t.is_idle_stopping(&id("api")));
        assert!(t.begin(&id("api")).is_some());
    }

    #[test]
    fn begin_all_takes_nothing_when_one_daemon_is_stopping() {
        let t = tracker();
        assert!(t.claim_idle_stop(&id("db"), Duration::ZERO));
        assert!(t.begin_all(&[id("api"), id("db")]).is_none());
        // The guard briefly taken on `api` was released again.
        assert_eq!(t.snapshot(&id("api")).in_flight, 0);
    }

    #[test]
    fn a_daemon_first_seen_by_a_snapshot_starts_a_fresh_idle_clock() {
        let t = tracker();
        let snap = t.snapshot(&id("adopted"));
        assert_eq!(snap.in_flight, 0);
        assert!(snap.idle_for < Duration::from_secs(1));
        assert!(!t.claim_idle_stop(&id("adopted"), Duration::from_secs(60)));
    }

    #[test]
    fn releasing_a_claim_restarts_the_idle_clock() {
        let t = tracker();
        assert!(t.claim_idle_stop(&id("api"), Duration::ZERO));
        t.release_idle_stop(&id("api"));
        assert!(!t.claim_idle_stop(&id("api"), Duration::from_secs(60)));
    }

    #[test]
    fn guarded_body_ends_activity_when_dropped() {
        let t = tracker();
        let body = GuardedBody::new(
            http_body_util::Empty::<hyper::body::Bytes>::new(),
            t.begin(&id("api")),
        );
        assert_eq!(t.snapshot(&id("api")).in_flight, 1);
        drop(body);
        assert_eq!(t.snapshot(&id("api")).in_flight, 0);
    }
}
