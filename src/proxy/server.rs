//! Reverse proxy server implementation.
//!
//! Listens on a configured port and routes requests to daemon processes based
//! on the `Host` header subdomain pattern.
//!
//! When `proxy.https = true`, a local CA is auto-generated (via `rcgen`) and
//! each incoming TLS connection is served with a per-domain certificate signed
//! by that CA (SNI-based dynamic certificate issuance).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use hyper::header::{COOKIE, HOST};

/// Response header used to identify a pitchfork proxy (for health checks and debugging).
const PITCHFORK_HEADER: &str = "x-pitchfork";

/// Request header tracking how many times a request has passed through the proxy.
/// Used to detect forwarding loops.
const PROXY_HOPS_HEADER: &str = "x-pitchfork-hops";

/// Maximum number of proxy hops before rejecting as a loop.
const MAX_PROXY_HOPS: u64 = 5;

/// How long a new connection has to reveal whether it is TLS and, if it is, to
/// finish the handshake.
///
/// Both steps happen before the connection is a request the server is willing
/// to spend time on, so a client that stalls there is holding a socket and a
/// task for nothing.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Maximum number of minted host certificates kept at once.
///
/// Every distinct SNI name under the TLD costs a key generation and a file, and
/// the set of names under a TLD is unbounded. A real machine serves a handful;
/// this is generous for that and still refuses to grow without limit when
/// something walks the namespace. The oldest entry is evicted, in memory and on
/// disk, so a busy name simply gets minted again.
const MAX_HOST_CERTS: usize = 256;

/// How often a refusal may be logged. See [`crate::proxy::LogThrottle`].
const REFUSAL_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Throttle for the "too many connections negotiating" warning.
static REFUSED_HANDSHAKE: crate::proxy::LogThrottle = crate::proxy::LogThrottle::new();

/// Throttle for the "refusing to issue a certificate" warning.
#[cfg(feature = "proxy-tls")]
static REFUSED_SNI: crate::proxy::LogThrottle = crate::proxy::LogThrottle::new();

/// Total time shutdown spends letting connections and tunnels finish.
///
/// Shared by both drain phases and kept inside what the supervisor waits for,
/// so neither phase is cut off by a caller that has stopped listening.
pub(crate) const SHUTDOWN_DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// Maximum number of `CONNECT` tunnels open at once.
///
/// A tunnel holds two sockets for as long as the client keeps it, and nothing
/// stops a client opening them in a loop — including through an existing
/// tunnel, since the far end is this same listener. The handshake budget does
/// not cover them, because a tunnel is established by then.
const MAX_TUNNELS: usize = 256;

/// Throttle for the "too many tunnels" warning.
static REFUSED_TUNNEL: crate::proxy::LogThrottle = crate::proxy::LogThrottle::new();

/// Throttle for connections abandoned before they said anything.
///
/// Debug-level, so silent by default, but `mise run install-dev` turns debug
/// logging on and these are triggered by exactly the behaviour their
/// warn-level siblings are throttled for: a client opening connections and
/// walking away. One line each would let it fill the disk of whoever is
/// debugging.
static ABANDONED_HANDSHAKE: crate::proxy::LogThrottle = crate::proxy::LogThrottle::new();

/// Throttle for `CONNECT` tunnels abandoned while being set up.
static ABANDONED_TUNNEL: crate::proxy::LogThrottle = crate::proxy::LogThrottle::new();

/// Maximum number of connections negotiating at once.
///
/// This bounds only protocol detection and the TLS handshake, both of which are
/// brief for a real client. A connection stops counting against it the moment
/// its handshake finishes, so long-lived sessions — keep-alive, HTTP/2,
/// WebSocket, CONNECT tunnels — do not consume the budget.
const MAX_PENDING_HANDSHAKES: usize = 512;

/// HTTP/1.1 hop-by-hop headers that are forbidden in HTTP/2 responses.
/// These must be stripped when proxying an HTTP/1.1 backend response back to an HTTP/2 client.
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
];

use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::net::{TcpListener, TcpStream};

use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::ProxyTlsMode;
use crate::settings::settings;
use crate::supervisor::SUPERVISOR;

// ─── Slug resolution cache ──────────────────────────────────────────────────
//
// `read_global_slugs()` reads ~/.config/pitchfork/config.toml from disk on every
// call, and `namespace_for_dir()` traverses the filesystem upward to find the
// nearest pitchfork.toml.  Both are called from `resolve_target_port()` which
// sits in the hot path of every proxied HTTP request.
//
// This cache stores the resolved slug → (namespace, daemon_name) mapping
// in memory with a short TTL so that the proxy does zero disk I/O for the vast
// majority of requests while still picking up config changes within seconds.

/// How long to cache the slug resolution table before re-reading from disk.
const SLUG_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// How a hostname's TLS is handled, and which daemon port it maps to.
///
/// Read from the daemon's `pitchfork.toml` (`proxy_tls`, `proxy_tls_port`) when
/// the slug table is refreshed, so the routing decision needs no disk I/O and
/// is available whether or not the daemon is currently running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProxyTlsRoute {
    /// `terminate` (default) or `passthrough`.
    pub mode: ProxyTlsMode,
    /// The daemon port this hostname maps to. `None` means the first port,
    /// which is what a single-port daemon always wants.
    pub port: Option<u16>,
}

/// Cached slug entry: pre-resolved namespace + daemon name for a slug.
#[derive(Clone, Debug)]
pub struct CachedSlugEntry {
    /// The slug key as registered in config (needed for display in auto-start pages).
    pub slug: String,
    /// Expected namespace derived from `entry.resolve_dir()` (None if derivation failed).
    pub namespace: Option<String>,
    /// Daemon short name (defaults to slug name when not explicitly set).
    pub daemon_name: String,
    /// Project directory for this slug (needed for auto-start).
    pub dir: std::path::PathBuf,
    /// Worktrees (git) / workspaces (jj) discovered under this slug's project directory.
    pub worktrees: Vec<crate::proxy::worktree::WorktreeEntry>,
    /// Sanitized worktree prefixes (ASCII-lowercased) that were discovered but
    /// are ambiguous, kept so a request naming one is refused rather than
    /// falling through to the parent slug as an unknown wildcard prefix.
    pub rejected_worktree_prefixes: std::collections::HashSet<String>,
    /// TLS route for the slug's main checkout.
    pub tls: ProxyTlsRoute,
    /// TLS routes per worktree, keyed by ASCII-lowercased sanitized branch.
    /// A worktree can configure its own `proxy_tls`, since each checkout has
    /// its own `pitchfork.toml`.
    pub worktree_tls: std::collections::HashMap<String, ProxyTlsRoute>,
}

impl CachedSlugEntry {
    /// The route this entry recorded for its main checkout, provided the slug
    /// still names the same daemon in the same directory and namespace. A slug
    /// repointed elsewhere must not carry the old daemon's mode and port over
    /// to a new target whose config cannot be read.
    fn known_route(
        &self,
        dir: &std::path::Path,
        namespace: Option<&str>,
        daemon_name: &str,
    ) -> Option<ProxyTlsRoute> {
        (self.dir == dir
            && self.namespace.as_deref() == namespace
            && self.daemon_name == daemon_name)
            .then_some(self.tls)
    }

    /// The route this entry recorded for a worktree, provided the same daemon
    /// was known there at the same path and namespace.
    fn known_worktree_route(
        &self,
        wt: &crate::proxy::worktree::WorktreeEntry,
        daemon_name: &str,
    ) -> Option<ProxyTlsRoute> {
        if self.daemon_name != daemon_name {
            return None;
        }
        let branch = wt.sanitized_branch.to_ascii_lowercase();
        self.worktrees
            .iter()
            .any(|known| {
                known.sanitized_branch.eq_ignore_ascii_case(&branch)
                    && known.path == wt.path
                    && known.namespace == wt.namespace
            })
            .then(|| self.worktree_tls.get(&branch).copied())
            .flatten()
    }
}

/// In-memory cache for the global slug registry + derived namespaces.
struct SlugCache {
    entries: Arc<std::collections::HashMap<String, CachedSlugEntry>>,
    expires_at: std::time::Instant,
}

/// The cached slug table.
///
/// A `std::sync::RwLock` rather than an async one on purpose: `rustls` calls
/// its certificate resolver from a synchronous trait method, and that resolver
/// has to know whether a hostname is passthrough before it issues a certificate
/// for it. One lock means routing and the resolver read the same table by
/// construction, so they cannot disagree about a hostname's mode. Every
/// critical section is a clone of an `Arc` or a comparison of two `Instant`s,
/// with the disk I/O deliberately outside, and nothing awaits while holding it.
static SLUG_CACHE: once_cell::sync::Lazy<std::sync::RwLock<SlugCache>> =
    once_cell::sync::Lazy::new(|| {
        std::sync::RwLock::new(SlugCache {
            entries: Arc::new(std::collections::HashMap::new()),
            expires_at: std::time::Instant::now(), // expired → will be populated on first access
        })
    });

/// Read the cached slug table without awaiting.
///
/// Empty until the table has been populated once, which every TLS connection
/// does before reaching the certificate resolver. An empty table resolves every
/// hostname to `terminate`, which is the behavior of a proxy with no slugs.
fn slug_snapshot() -> Arc<std::collections::HashMap<String, CachedSlugEntry>> {
    let guard = SLUG_CACHE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(&guard.entries)
}

/// Serializes refreshes of [`SLUG_CACHE`], so at most one build is in flight.
///
/// Without it, every request arriving on an expired cache builds its own table
/// and publishes it, which both duplicates the work — the build runs
/// subprocesses to discover worktrees — and leaves each caller holding a
/// different generation than the one the certificate resolver reads. With it,
/// the first arrival builds and the rest wait and take that table, so one
/// generation is live at a time.
static SLUG_REFRESH: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// The cached table if it has not expired.
fn fresh_slugs() -> Option<Arc<std::collections::HashMap<String, CachedSlugEntry>>> {
    let cache = SLUG_CACHE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (std::time::Instant::now() < cache.expires_at).then(|| Arc::clone(&cache.entries))
}

/// Drop every worktree whose sanitized branch is ambiguous under
/// case-insensitive host matching, keeping the unambiguous ones.
///
/// Both sides of a collision are dropped rather than one being picked: the
/// alternative routes a request to a worktree the user did not name, which is
/// worse than not routing it at all.
fn reject_case_colliding_worktrees(
    wts: Vec<crate::proxy::worktree::WorktreeEntry>,
) -> (
    Vec<crate::proxy::worktree::WorktreeEntry>,
    std::collections::HashSet<String>,
) {
    let collisions =
        crate::proxy::ascii_case_collisions(wts.iter().map(|w| w.sanitized_branch.as_str()));
    if collisions.is_empty() {
        return (wts, collisions);
    }

    let (dropped, kept): (Vec<_>, Vec<_>) = wts
        .into_iter()
        .partition(|w| collisions.contains(&w.sanitized_branch.to_ascii_lowercase()));

    let mut folded: Vec<&String> = collisions.iter().collect();
    folded.sort();
    for key in folded {
        let mut branches: Vec<&str> = dropped
            .iter()
            .filter(|w| w.sanitized_branch.eq_ignore_ascii_case(key))
            .map(|w| w.branch.as_str())
            .collect();
        branches.sort();
        log::warn!(
            "Worktree slug collision: branches [{}] all route to '{key}' under \
             case-insensitive host matching. None of them will be routed; \
             rename a branch to disambiguate.",
            branches.join(", "),
        );
    }

    (kept, collisions)
}

/// Read a daemon's TLS route (`proxy_tls`, `proxy_tls_port`) from the config
/// that applies in `dir`.
///
/// `Ok(None)` means that directory says nothing about the daemon: its config
/// does not describe it. That is deliberately not the same as a route of
/// `terminate`: a worktree the proxy knows nothing about inherits its slug's
/// mode, where a synthesized `terminate` would silently stop splicing a
/// passthrough daemon and answer with the proxy's certificate instead. A
/// directory that *does* describe the daemon is authoritative, including when
/// it leaves `proxy_tls` out and so terminates.
///
/// `Err` means the config could not be read at all, which says nothing about
/// the daemon either way. The merged config fails as a whole, so an error in
/// an unrelated daemon lands here too; callers that already know a route keep
/// it rather than treat the error as `terminate`.
///
/// Any other way of resolving a hostname to a daemon can reuse this to get the
/// same TLS decision, since the mode belongs to the daemon rather than to the
/// hostname that reached it.
///
/// The config read goes through the merged-config cache, so a refresh that
/// finds nothing changed costs a `stat` per config file rather than a reparse.
pub(crate) fn read_proxy_tls_route(
    dir: &std::path::Path,
    namespace: Option<&str>,
    daemon_name: &str,
) -> miette::Result<Option<ProxyTlsRoute>> {
    let Some(id) = namespace.and_then(|ns| DaemonId::try_new(ns, daemon_name).ok()) else {
        return Ok(None);
    };
    let pt = crate::pitchfork_toml::PitchforkToml::all_merged_from(dir)?;
    Ok(pt.daemons.get(&id).map(|cfg| ProxyTlsRoute {
        mode: cfg.proxy_tls.unwrap_or_default(),
        port: cfg.effective_proxy_tls_port(),
    }))
}

/// The result of [`read_proxy_tls_route`], falling back to `known` — the route
/// recorded by the previous slug-table refresh — when the config is unreadable.
///
/// The warning is logged once per distinct error, since the table refreshes
/// every couple of seconds and the error persists until the config is fixed.
fn route_or_last_known(
    read: miette::Result<Option<ProxyTlsRoute>>,
    known: Option<ProxyTlsRoute>,
    dir: &std::path::Path,
    daemon_name: &str,
) -> Option<ProxyTlsRoute> {
    read.unwrap_or_else(|e| {
        crate::proxy::hostname::warn_once(&format!(
            "Proxy TLS route for daemon '{daemon_name}': could not read config in {}; \
             keeping its last known TLS mode until the config is fixed: {e}",
            dir.display()
        ));
        known
    })
}

/// Build the slug lookup table from disk (expensive — involves file I/O + subprocesses).
/// Called outside the cache lock via `spawn_blocking` to avoid blocking the Tokio runtime.
///
/// Keys are ASCII-lowercased, and slugs that collide once folded are left out
/// entirely — see [`reject_case_colliding_worktrees`] for why ambiguity is
/// rejected rather than resolved.
///
/// `previous` is the table being replaced. When a directory's config cannot be
/// read, the route recorded there is kept, so a config error — even one in an
/// unrelated daemon — cannot quietly turn a running passthrough daemon into a
/// terminated one that answers with the proxy's certificate.
fn build_slug_entries(
    previous: &std::collections::HashMap<String, CachedSlugEntry>,
) -> std::collections::HashMap<String, CachedSlugEntry> {
    let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    let collisions = crate::proxy::ascii_case_collisions(global_slugs.keys().map(String::as_str));
    let mut folded: Vec<&String> = collisions.iter().collect();
    folded.sort();
    for key in folded {
        let mut spellings: Vec<&str> = global_slugs
            .keys()
            .filter(|s| s.eq_ignore_ascii_case(key))
            .map(String::as_str)
            .collect();
        spellings.sort();
        log::warn!(
            "Slug collision: [{}] differ only by case and host names are case-insensitive. \
             None of them will be routed; remove or rename all but one.",
            spellings.join(", "),
        );
    }

    let mut entries: std::collections::HashMap<String, CachedSlugEntry> =
        std::collections::HashMap::with_capacity(global_slugs.len());
    let worktree_enabled = crate::settings::settings().general.worktree;
    for (slug, entry) in &global_slugs {
        let key = slug.to_ascii_lowercase();
        if collisions.contains(&key) {
            continue;
        }
        let ns = entry.resolve_namespace();
        let daemon_name = entry.daemon.as_deref().unwrap_or(slug).to_string();
        let (worktrees, rejected_worktree_prefixes) = if worktree_enabled {
            let wts = match entry.resolve_dir() {
                Some(dir) => crate::proxy::worktree::discover_worktrees(&dir),
                None => vec![],
            };
            let wts = wts
                .into_iter()
                .map(|mut wt| {
                    wt.namespace =
                        crate::pitchfork_toml::PitchforkToml::namespace_for_dir(&wt.path).ok();
                    wt
                })
                .collect();
            reject_case_colliding_worktrees(wts)
        } else {
            (vec![], std::collections::HashSet::new())
        };
        let dir = entry.resolve_dir().unwrap_or_default();
        let prev = previous.get(&key);
        // The slug's own directory has nothing to inherit from, so silence
        // there is the default route. An unreadable config keeps the last
        // known route; with none known yet, there is nothing else to go on.
        let tls = route_or_last_known(
            read_proxy_tls_route(&dir, ns.as_deref(), &daemon_name),
            prev.and_then(|p| p.known_route(&dir, ns.as_deref(), &daemon_name)),
            &dir,
            &daemon_name,
        )
        .unwrap_or_default();
        // A worktree only gets an entry when its own config describes the
        // daemon, or when it did last time and cannot be read now; the rest
        // inherit the slug's route at lookup time.
        let worktree_tls = worktrees
            .iter()
            .filter_map(|wt| {
                let branch = wt.sanitized_branch.to_ascii_lowercase();
                route_or_last_known(
                    read_proxy_tls_route(&wt.path, wt.namespace.as_deref(), &daemon_name),
                    prev.and_then(|p| p.known_worktree_route(wt, &daemon_name)),
                    &wt.path,
                    &daemon_name,
                )
                .map(|route| (branch, route))
            })
            .collect();
        entries.insert(
            key,
            CachedSlugEntry {
                slug: slug.clone(),
                namespace: ns,
                daemon_name,
                dir,
                worktrees,
                rejected_worktree_prefixes,
                tls,
                worktree_tls,
            },
        );
    }
    entries
}

/// Return the cached slug table, refreshing from disk if expired.
///
/// The disk I/O happens *outside* the cache lock so a refresh does not block
/// readers, and refreshes are serialized against each other so only one build
/// runs at a time. Every caller therefore returns the one table that is
/// published, which is what keeps routing and the certificate resolver from
/// disagreeing about a hostname: a caller holding its own build could route a
/// hostname as terminating while the resolver, reading the published table,
/// refuses to issue for it and the handshake is dropped rather than spliced.
pub async fn get_cached_slugs() -> Arc<std::collections::HashMap<String, CachedSlugEntry>> {
    // Fast path: cache still valid — just clone the Arc.
    if let Some(entries) = fresh_slugs() {
        return entries;
    }

    // Slow path: one refresh at a time.
    let _refreshing = SLUG_REFRESH.lock().await;

    // Another caller may have refreshed while this one waited for the lock.
    if let Some(entries) = fresh_slugs() {
        return entries;
    }

    // Build from disk on a blocking thread (involves subprocess calls).
    let previous = slug_snapshot();
    let new_entries = Arc::new(
        tokio::task::spawn_blocking(move || build_slug_entries(&previous))
            .await
            .unwrap_or_else(|e| {
                log::warn!("Failed to refresh slug cache: {e}");
                std::collections::HashMap::new()
            }),
    );

    let mut cache = SLUG_CACHE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.entries = Arc::clone(&new_entries);
    cache.expires_at = std::time::Instant::now() + SLUG_CACHE_TTL;
    new_entries
}

// ─── Hostname registry cache ────────────────────────────────────────────────
//
// The automatic `<daemon>.<worktree>.<project>` hostnames are resolved against
// a registry built from every project pitchfork knows about.  Building it reads
// configuration files and enumerates git worktrees, so the result is cached
// with the same short TTL as the slug table.

struct RegistryCache {
    registry: Arc<crate::proxy::hostname::HostRegistry>,
    expires_at: std::time::Instant,
}

/// The cached hostname registry.
///
/// A `std::sync::RwLock` for the same reason as [`SLUG_CACHE`]: the certificate
/// resolver runs in a synchronous trait method and has to know whether a
/// hostname is passthrough before it issues a certificate for it. Both readers
/// share one registry, so routing and the resolver cannot disagree.
static HOST_REGISTRY: once_cell::sync::Lazy<std::sync::RwLock<RegistryCache>> =
    once_cell::sync::Lazy::new(|| {
        std::sync::RwLock::new(RegistryCache {
            registry: Arc::new(crate::proxy::hostname::HostRegistry::default()),
            expires_at: std::time::Instant::now(), // expired -> built on first access
        })
    });

/// Serializes registry refreshes, so one build runs at a time and every caller
/// returns the table that was published. See [`SLUG_REFRESH`].
static REGISTRY_REFRESH: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// The cached registry if it has not expired.
fn fresh_registry() -> Option<Arc<crate::proxy::hostname::HostRegistry>> {
    let cache = HOST_REGISTRY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (std::time::Instant::now() < cache.expires_at).then(|| Arc::clone(&cache.registry))
}

/// Read the cached hostname registry without awaiting.
///
/// Empty until it has been built once, which every TLS connection does before
/// reaching the certificate resolver. An empty registry resolves every hostname
/// to `terminate`, the behavior of a proxy that knows no projects.
fn registry_snapshot() -> Arc<crate::proxy::hostname::HostRegistry> {
    let cache = HOST_REGISTRY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(&cache.registry)
}

/// Return a snapshot of the cached hostname registry, rebuilding if expired.
pub async fn get_cached_host_registry() -> Arc<crate::proxy::hostname::HostRegistry> {
    if let Some(registry) = fresh_registry() {
        return registry;
    }

    // One refresh at a time; the rest wait and take its result.
    let _refreshing = REGISTRY_REFRESH.lock().await;
    if let Some(registry) = fresh_registry() {
        return registry;
    }

    let registry = Arc::new(
        tokio::task::spawn_blocking(crate::proxy::hostname::HostRegistry::build)
            .await
            .unwrap_or_else(|e| {
                log::warn!("Failed to refresh hostname registry: {e}");
                crate::proxy::hostname::HostRegistry::default()
            }),
    );
    for err in &registry.errors {
        crate::proxy::hostname::warn_once(err);
    }

    let mut cache = HOST_REGISTRY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.registry = Arc::clone(&registry);
    cache.expires_at = std::time::Instant::now() + SLUG_CACHE_TTL;
    registry
}

/// Try to match a subdomain against a slug table, with optional wildcard fallback.
///
/// When `wildcard` is true and no exact match is found, progressively strips
/// subdomain prefixes from the left until a match is found or no dots remain.
/// For example, with slug "myapp" registered, `tenant.myapp` matches "myapp".
///
/// `entries` must be keyed by the ASCII-lowercased slug, as
/// [`build_slug_entries`] produces: host names are case-insensitive (RFC 4343),
/// so the subdomain is lowercased before every lookup.
fn wildcard_slug_lookup<'a>(
    subdomain: &str,
    entries: &'a std::collections::HashMap<String, CachedSlugEntry>,
    wildcard: bool,
) -> Option<&'a CachedSlugEntry> {
    let subdomain = subdomain.to_ascii_lowercase();

    entries.get(&subdomain).or_else(|| {
        if !wildcard {
            return None;
        }
        // "a.b.myapp" has dots at 1,3 → "b.myapp", "myapp"
        subdomain
            .match_indices('.')
            .map(|(i, _)| &subdomain[i + 1..])
            .find_map(|candidate| entries.get(candidate))
    })
}

/// What a wildcard subdomain prefix resolves to within a slug's worktrees.
#[derive(Debug)]
enum PrefixMatch<'a> {
    /// The prefix names exactly one discovered worktree.
    Worktree(&'a crate::proxy::worktree::WorktreeEntry),
    /// The prefix names no worktree — an ordinary wildcard subdomain, served
    /// by the slug's main checkout.
    Unknown,
    /// The prefix names worktrees that were rejected as ambiguous.  Serving the
    /// main checkout here would answer successfully with the wrong content, so
    /// the request is refused instead.
    Ambiguous,
}

/// Resolve a wildcard subdomain prefix against a slug's cached worktrees.
fn match_worktree_prefix<'a>(cached: &'a CachedSlugEntry, prefix: &str) -> PrefixMatch<'a> {
    if let Some(wt) = cached
        .worktrees
        .iter()
        .find(|w| w.sanitized_branch.eq_ignore_ascii_case(prefix))
    {
        return PrefixMatch::Worktree(wt);
    }
    if cached
        .rejected_worktree_prefixes
        .contains(&prefix.to_ascii_lowercase())
    {
        return PrefixMatch::Ambiguous;
    }
    PrefixMatch::Unknown
}

/// The TLS route for a worktree of `cached`.
///
/// A worktree whose own config describes the daemon is authoritative. One the
/// proxy knows nothing about — an unreadable or momentarily invalid config, a
/// checkout that predates the daemon — inherits the slug's *mode* but not its
/// port.
///
/// Inheriting the mode is what keeps a passthrough slug from quietly being
/// terminated with the proxy's certificate while its config is unreadable. The
/// port is deliberately not inherited: `proxy_tls_port` names a position in one
/// daemon's port list, and the daemon behind a worktree hostname is a different
/// process with its own ports, so the hostname falls back to that daemon's own
/// first port rather than to a number chosen for another checkout.
fn worktree_route(cached: &CachedSlugEntry, sanitized_branch: &str) -> ProxyTlsRoute {
    cached
        .worktree_tls
        .get(&sanitized_branch.to_ascii_lowercase())
        .copied()
        .unwrap_or(ProxyTlsRoute {
            mode: cached.tls.mode,
            port: None,
        })
}

/// Strip a trailing `.{suffix}` from `s`, ignoring ASCII case.
///
/// Returns the remaining prefix, or `None` when `s` does not end that way.
fn strip_dot_suffix_ignore_case(s: &str, suffix: &str) -> Option<String> {
    let needle_len = suffix.len() + 1;
    if s.len() <= needle_len {
        return None;
    }
    let split = s.len() - needle_len;
    if !s.is_char_boundary(split) {
        return None;
    }
    let (head, tail) = s.split_at(split);
    if tail.starts_with('.') && tail[1..].eq_ignore_ascii_case(suffix) {
        Some(head.to_string())
    } else {
        None
    }
}

/// Look up a slug in the cached table.
///
/// With wildcard enabled (default), falls back to progressively shorter
/// subdomain suffixes when an exact match is not found.  For example,
/// `tenant.myapp` will match slug `myapp` if no slug named `tenant.myapp`
/// exists.
async fn cached_slug_lookup(subdomain: &str) -> Option<CachedSlugEntry> {
    let entries = get_cached_slugs().await;
    wildcard_slug_lookup(subdomain, &entries, settings().proxy.wildcard).cloned()
}

// ─── Auto-start deduplication ───────────────────────────────────────────────
//
// When auto_start is enabled, concurrent proxy requests for the same stopped
// daemon must not trigger multiple start operations.  This set tracks daemon
// IDs that are currently being auto-started.

static AUTO_START_IN_PROGRESS: once_cell::sync::Lazy<
    tokio::sync::Mutex<std::collections::HashSet<DaemonId>>,
> = once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

/// Result of resolving a proxy target for a given host.
enum ResolveResult {
    /// Daemon is running and ready — forward to this port.
    /// Covers both already-running daemons and freshly auto-started ones.
    Ready(u16),
    /// Daemon is currently starting (auto-start in progress or just triggered).
    Starting { slug: String },
    /// No matching slug or daemon found.
    NotFound,
    /// The hostname is reserved for a project or stack page, which a later
    /// change will serve.  It must never fall through to a daemon.
    Page {
        project: String,
        worktree: Option<String>,
        daemons: Vec<String>,
        /// The checkout the hostname resolved to, which is what maps the page
        /// back to its URL in the web UI: hostname labels are sanitized and can
        /// be overridden, so they are not the names those URLs use.
        dir: Option<std::path::PathBuf>,
    },
    /// The hostname named a project or daemon that does not exist.
    Unknown { heading: String, known: Vec<String> },
    /// Routing refused with a descriptive reason.
    Error(String),
}

/// Shared proxy state passed to each request handler.
/// Callback type invoked on proxy errors (e.g. for logging/alerting).
type OnErrorFn = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
struct ProxyState {
    /// HTTP client used to forward requests to daemon backends.
    client: Arc<Client<HttpConnector, Body>>,
    /// The configured TLD (e.g. "localhost").
    tld: String,
    /// Whether the proxy is serving HTTPS.
    is_tls: bool,
    /// Address a `CONNECT` tunnel is spliced to: this proxy's own listener.
    connect_target: Option<SocketAddr>,
    /// Address a client on this machine uses to reach the listener, which the
    /// PAC file names.
    contact_ip: std::net::IpAddr,
    /// Cancelled when the proxy is shutting down.
    cancel: tokio_util::sync::CancellationToken,
    /// Budget for concurrent `CONNECT` tunnels.
    tunnel_slots: Arc<tokio::sync::Semaphore>,
    /// Live `CONNECT` tunnels.
    ///
    /// A tunnel outlives the request that created it, so it cannot live in the
    /// connection `JoinSet`. Tracking it here lets shutdown cancel the tunnels
    /// and then wait for them, rather than returning while one still owns both
    /// of its sockets.
    tunnels: tokio_util::task::TaskTracker,
    /// Optional error callback invoked on proxy errors (e.g. for logging/alerting).
    on_error: Option<OnErrorFn>,
}

/// Start the reverse proxy server.
///
/// Binds to the configured port and serves until the process exits.
/// When `proxy.https = true`, TLS is terminated here using a self-signed
/// certificate (auto-generated if not present).
///
/// This function is intended to be spawned as a background task.
pub async fn serve(
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let s = settings();
    let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();

    let effective_tld = crate::proxy::effective_tld(&s).to_string();

    let Some(effective_port) = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) else {
        let msg = format!(
            "proxy.port {} is out of valid port range (1-65535), proxy server cannot start",
            s.proxy.port
        );
        let _ = bind_tx.send(Err(msg.clone()));
        miette::bail!("{msg}");
    };

    let mut connector = HttpConnector::new();
    // Limit how long the proxy waits to establish a TCP connection to a backend.
    // Without this, a daemon that accepts the SYN but never completes the handshake
    // would stall the proxy indefinitely.
    connector.set_connect_timeout(Some(std::time::Duration::from_secs(10)));

    let client = Client::builder(TokioExecutor::new())
        // Reclaim idle keep-alive connections after 30 s so that file descriptors
        // are not held open forever when a backend goes quiet.
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build(connector);

    // Resolve bind address from settings.
    // In LAN mode, default to 0.0.0.0 so the proxy is reachable from other
    // devices on the network.  Users can still override with proxy.host.
    let bind_ip: std::net::IpAddr = if lan_enabled && s.proxy.host == "127.0.0.1" {
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
    } else {
        match s.proxy.host.parse() {
            Ok(ip) => ip,
            Err(_) => {
                log::warn!(
                    "proxy.host {:?} is not a valid IP address — falling back to 127.0.0.1. \
                     The proxy will only be reachable on the loopback interface.",
                    s.proxy.host
                );
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            }
        }
    };
    let addr = SocketAddr::from((bind_ip, effective_port));
    // The address a local client should use to reach this listener. Derived
    // from the bind address rather than assumed to be 127.0.0.1: with
    // `proxy.host = "::1"` nothing is listening on IPv4 at all, so both the PAC
    // file and the CONNECT tunnel have to name the IPv6 loopback instead.
    let contact_ip = local_contact_ip(bind_ip);
    let tunnels = tokio_util::task::TaskTracker::new();

    let state = ProxyState {
        client: Arc::new(client),
        tld: effective_tld.clone(),
        is_tls: s.proxy.https,
        // CONNECT tunnels loop back into this same listener, so the TLS
        // handshake inside the tunnel reaches the SNI resolver.
        connect_target: Some(SocketAddr::from((contact_ip, effective_port))),
        contact_ip,
        cancel: cancel.clone(),
        tunnel_slots: Arc::new(tokio::sync::Semaphore::new(MAX_TUNNELS)),
        tunnels: tunnels.clone(),
        on_error: None,
    };

    // `/proxy.pac` is served from the proxy's own listener because that is the
    // one HTTP endpoint that always exists while the proxy runs. The handler
    // falls through to normal proxying when the request is addressed to a
    // hostname under the TLD, so a daemon can still own that path.
    let plain_state = state.clone();
    let app = Router::new()
        // `any`, not `get`: axum answers a path match with no method match at
        // the router, before `fallback` runs. Registering GET alone would turn
        // a POST to `/proxy.pac` on *any* proxied hostname into a 405 that the
        // backend daemon never sees, which is exactly the fall-through the
        // comment above promises. The handler decides the method instead.
        .route(crate::proxy::pac::PAC_PATH, axum::routing::any(pac_handler))
        .fallback(proxy_handler)
        .with_state(state);

    if s.proxy.https {
        serve_https_with_http_fallback(
            app,
            addr,
            &s,
            effective_port,
            effective_tld,
            plain_state,
            bind_tx,
            cancel,
        )
        .await
    } else {
        // `plain_state` carries the tunnel tracker, which this path has to
        // drain too: `proxy.https = false` still serves CONNECT through the
        // PAC file, for `http://` and `ws://` URLs.
        serve_http(app, addr, effective_port, plain_state, bind_tx, cancel).await
    }
}

/// Serve plain HTTP.
async fn serve_http(
    app: Router,
    addr: SocketAddr,
    effective_port: u16,
    proxy_state: ProxyState,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {
            if settings().proxy.sync_hosts {
                crate::proxy::hosts::sync_hosts_from_settings();
            }
            let _ = bind_tx.send(Ok(()));
            l
        }
        Err(e) => {
            let msg = bind_error_message(effective_port, &e);
            let _ = bind_tx.send(Err(msg.clone()));
            return Err(miette::miette!("{msg}"));
        }
    };

    log::info!("Proxy server listening on http://{addr}");
    if effective_port < 1024 {
        log::info!(
            "Note: port {effective_port} is a privileged port. \
             The supervisor must be started with sudo to bind to this port."
        );
    }
    let shutdown_signal = cancel.clone().cancelled_owned();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal)
    .into_future();
    tokio::pin!(server);

    // Axum's graceful shutdown waits for every open connection with no limit,
    // and a long-lived `ws://` stream may never close on its own. One deadline,
    // started when shutdown begins, covers that wait and the tunnel drain
    // below, as the HTTPS path does; past it the remaining connections are
    // dropped rather than holding the supervisor's shutdown up.
    let deadline = tokio::select! {
        r = &mut server => {
            r.map_err(|e| miette::miette!("Proxy server error: {e}"))?;
            tokio::time::Instant::now() + SHUTDOWN_DRAIN_BUDGET
        }
        _ = cancel.cancelled() => {
            let deadline = tokio::time::Instant::now() + SHUTDOWN_DRAIN_BUDGET;
            match tokio::time::timeout_at(deadline, &mut server).await {
                Ok(r) => r.map_err(|e| miette::miette!("Proxy server error: {e}"))?,
                Err(_) => log::debug!(
                    "Proxy connections still open after {SHUTDOWN_DRAIN_BUDGET:?}; dropping them"
                ),
            }
            deadline
        }
    };

    // Axum's graceful shutdown returns once its own per-connection futures are
    // done, and a CONNECT request's future completes the moment the connection
    // is upgraded. The tunnel it spawned outlives it, so without this the
    // supervisor would go on to stop daemons while tunnels were still splicing
    // bytes.
    proxy_state.tunnels.close();
    let _ = tokio::time::timeout_at(deadline, proxy_state.tunnels.wait()).await;
    Ok(())
}

/// Serve HTTPS with automatic HTTP detection on the same port.
///
/// Peeks at the first byte of each incoming TCP connection:
/// - `0x16` (TLS ClientHello) → hand off to the TLS acceptor (HTTP/2 + HTTP/1.1 via ALPN)
/// - anything else → 302 redirect to HTTPS
#[cfg(feature = "proxy-tls")]
#[allow(clippy::too_many_arguments)]
async fn serve_https_with_http_fallback(
    app: Router,
    addr: SocketAddr,
    s: &crate::settings::Settings,
    effective_port: u16,
    effective_tld: String,
    plain_proxy_state: ProxyState,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    use rustls::ServerConfig;
    use tokio_rustls::TlsAcceptor;

    let (cert_path, key_path) = resolve_tls_paths(s)?;

    // Install ring as the default CryptoProvider if none has been set yet.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // A configured `tls_cert` is served as-is: the user supplied a certificate
    // for these host names, so pitchfork has no CA to mint from and no business
    // replacing what they chose.  Otherwise the local CA signs a leaf per SNI
    // host on demand, which is what makes names of any depth work.
    let resolver: Arc<dyn rustls::server::ResolvesServerCert> = if s.proxy.tls_cert.is_empty() {
        if ensure_ca(&cert_path, &key_path, || {
            cert_path.exists() && key_path.exists()
        })? {
            log::info!("Generated local CA certificate at {}", cert_path.display());
            log::info!("To trust the CA in your browser, run: pitchfork proxy trust");
        }
        Arc::new(SniCertResolver::new(
            &cert_path,
            &key_path,
            effective_tld.clone(),
        )?)
    } else {
        log::info!(
            "Serving the configured certificate {} (no certificates are minted)",
            cert_path.display()
        );
        Arc::new(StaticCertResolver::new(
            &cert_path,
            &key_path,
            effective_tld.clone(),
        )?)
    };

    let mut tls_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    // Advertise HTTP/2 and HTTP/1.1 via ALPN so browsers negotiate HTTP/2
    // for multiplexed requests (eliminates the 6-connection-per-host limit).
    tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {
            if settings().proxy.sync_hosts {
                crate::proxy::hosts::sync_hosts_from_settings();
            }
            let _ = bind_tx.send(Ok(()));
            l
        }
        Err(e) => {
            let msg = bind_error_message(effective_port, &e);
            let _ = bind_tx.send(Err(msg.clone()));
            return Err(miette::miette!("{msg}"));
        }
    };

    log::info!("Proxy server listening on https://{addr} (HTTP also accepted)");
    if effective_port < 1024 {
        log::info!(
            "Note: port {effective_port} is a privileged port. \
             The supervisor must be started with sudo to bind to this port."
        );
    }

    // Build a lightweight app for plain-HTTP requests arriving on the TLS port.
    // Two things are exempt from the redirect to HTTPS:
    //
    //   * `/proxy.pac`, because the browser reads the PAC file before it has
    //     any way to trust this listener's certificate.
    //   * `CONNECT`, which is how a PAC-configured browser opens an `https://`
    //     URL. Redirecting it would break the PAC path entirely.
    let redirect_app = Router::new()
        .route(
            // `any` for the same reason as the TLS side: the method check
            // belongs in the handler, so a proxied host keeps this path.
            crate::proxy::pac::PAC_PATH,
            axum::routing::any(plain_pac_handler),
        )
        .fallback(plain_fallback_handler)
        .with_state(PlainState {
            tld: effective_tld.clone(),
            port: effective_port,
            proxy: plain_proxy_state.clone(),
        });

    // Accept connections and sniff the first byte to decide TLS vs plain HTTP.
    let mut conn_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let handshake_slots = Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_HANDSHAKES));
    loop {
        // Reap finished connection tasks during normal operation so the JoinSet
        // does not retain one entry per historical connection.
        while conn_tasks.try_join_next().is_some() {}

        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, peer_addr) = match accept_result {
                    Ok(conn) => conn,
                    Err(e) => {
                        log::warn!("Accept error (will retry): {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };

                // Refuse rather than queue without bound. A client can hold a
                // connection in protocol detection or mid-handshake by simply
                // not sending, and each one costs a task and a descriptor.
                //
                // The permit covers the handshake only and is released before
                // the connection is served, so established sessions never
                // occupy the budget.
                let Ok(handshake_permit) = Arc::clone(&handshake_slots).try_acquire_owned() else {
                    // Throttled: whoever is saturating the handshake budget can
                    // provoke this line as fast as it can open sockets.
                    if let Some(suppressed) = REFUSED_HANDSHAKE.allow(REFUSAL_LOG_INTERVAL) {
                        log::warn!(
                            "Proxy refused a connection: {MAX_PENDING_HANDSHAKES} \
                             still negotiating \
                             ({suppressed} similar refusals since the last message)"
                        );
                    }
                    drop(stream);
                    continue;
                };

                let acceptor = acceptor.clone();
                // This loop serves the router itself rather than going through
                // `into_make_service_with_connect_info`, so the peer address is
                // attached here. Handlers use it to decide how much of this
                // machine's configuration a response may describe.
                let app = app
                    .clone()
                    .layer(axum::Extension(axum::extract::ConnectInfo(peer_addr)));
                let redirect_app = redirect_app.clone();
                let tld = effective_tld.clone();

                conn_tasks.spawn(async move {
                    // Peek at the first byte without consuming it.
                    // TLS ClientHello always starts with 0x16 (content type "handshake").
                    //
                    // One deadline covers the peek and the handshake together,
                    // so a client that trickles its first byte in just in time
                    // does not start a second full budget.
                    let handshake_deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
                    let mut peek_buf = [0u8; 1];
                    match tokio::time::timeout_at(handshake_deadline, stream.peek(&mut peek_buf)).await {
                        Ok(Ok(0)) | Ok(Err(_)) => return,
                        Err(_) => {
                            if let Some(suppressed) =
                                ABANDONED_HANDSHAKE.allow(REFUSAL_LOG_INTERVAL)
                            {
                                log::debug!(
                                    "Connection sent nothing within the handshake timeout \
                                     ({suppressed} similar since the last message)"
                                );
                            }
                            return;
                        }
                        Ok(Ok(_)) => {}
                    }

                    if peek_buf[0] == 0x16 {
                        // A TLS connection whose SNI names a `proxy_tls = "passthrough"`
                        // daemon is spliced through untouched, so the daemon's own
                        // certificate, ALPN and client-certificate request reach the
                        // client. Everything else is terminated here as before.
                        //
                        // The peek counts against the same handshake deadline:
                        // it is still negotiation, not a session.
                        let sni_budget = SNI_PEEK_TIMEOUT.min(
                            handshake_deadline.saturating_duration_since(tokio::time::Instant::now()),
                        );
                        match peek_sni_host(&stream, sni_budget).await {
                            SniProbe::Host(host) => {
                                if resolve_tls_mode(&host, &tld).await.is_passthrough() {
                                    // The splice is the session, however long
                                    // it lasts; it must not keep a slot meant
                                    // for connections still negotiating.
                                    drop(handshake_permit);
                                    serve_passthrough(stream, &host, &tld).await;
                                    return;
                                }
                            }
                            SniProbe::NoHost => {}
                            // The hostname could not be read here, so this
                            // connection is handed to the TLS acceptor like any
                            // other: rustls defragments the handshake itself
                            // and hands the real host name to the certificate
                            // resolver, which refuses to issue for a
                            // passthrough hostname rather than answering with
                            // the proxy's certificate. Unrelated terminating
                            // hostnames are served normally.
                            SniProbe::Undetermined => {
                                log::debug!(
                                    "Could not read the ClientHello of a TLS connection; \
                                     handing it to the TLS acceptor, which refuses to terminate \
                                     a passthrough hostname."
                                );
                                // Make sure the resolver's synchronous snapshots
                                // have been populated before it has to decide.
                                let _ = get_cached_slugs().await;
                                let _ = get_cached_host_registry().await;
                            }
                        }

                        // TLS handshake → HTTP/2 or HTTP/1.1 (negotiated via ALPN)
                        let accepted = match tokio::time::timeout_at(
                            handshake_deadline,
                            acceptor.accept(stream),
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                if let Some(suppressed) =
                                    ABANDONED_HANDSHAKE.allow(REFUSAL_LOG_INTERVAL)
                                {
                                    log::debug!(
                                        "TLS handshake did not complete in time \
                                         ({suppressed} similar since the last message)"
                                    );
                                }
                                return;
                            }
                        };
                        // Negotiation is over either way; the session that
                        // follows can last as long as it likes.
                        drop(handshake_permit);
                        match accepted {
                            Ok(tls_stream) => {
                                let io = hyper_util::rt::TokioIo::new(tls_stream);
                                let svc = hyper_util::service::TowerToHyperService::new(app);
                                if let Err(e) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                                    .serve_connection_with_upgrades(io, svc)
                                    .await
                                {
                                    // HTTP/2 RST_STREAM errors from cancelled browser requests
                                    // (navigation, HMR) are normal — log at debug to avoid noise.
                                    log::debug!("Connection error: {e}");
                                }
                            }
                            Err(e) => {
                                log::debug!("TLS handshake error: {e}");
                            }
                        }
                    } else {
                        // Plain HTTP on the TLS port → 302 redirect to HTTPS.
                        // Nothing left to negotiate.
                        drop(handshake_permit);
                        let io = hyper_util::rt::TokioIo::new(stream);
                        let svc = hyper_util::service::TowerToHyperService::new(redirect_app);
                        let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                            .serve_connection_with_upgrades(io, svc)
                            .await;
                    }
                });

                while conn_tasks.try_join_next().is_some() {}
            }
            _ = cancel.cancelled() => {
                log::info!("Proxy server shutting down (cancel signal received)");
                break;
            }
        }
    }

    // One budget for both drains, not one each. The supervisor waits a fixed
    // time for this task; two independent timeouts could together outlast it,
    // so the second phase would be cut off rather than bounded.
    let deadline = tokio::time::Instant::now() + SHUTDOWN_DRAIN_BUDGET;

    // In-flight connections first.
    let _ = tokio::time::timeout_at(deadline, async {
        while conn_tasks.join_next().await.is_some() {}
    })
    .await;

    // Then the CONNECT tunnels. They were cancelled along with everything else
    // when the token fired, so this is waiting for them to let go of their
    // sockets rather than waiting for their peers to finish.
    plain_proxy_state.tunnels.close();
    let _ = tokio::time::timeout_at(deadline, plain_proxy_state.tunnels.wait()).await;

    Ok(())
}

/// How long to wait for a complete ClientHello before giving up on reading SNI.
///
/// A client sends its ClientHello immediately after the TCP handshake, so this
/// only ever expires on a stalled or malicious connection.
#[cfg(feature = "proxy-tls")]
const SNI_PEEK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How much of the connection's opening bytes to examine while looking for SNI.
///
/// A ClientHello with a realistic extension set is well under 2 KiB; the cap
/// bounds the work done for a connection that never sends a parseable one.
#[cfg(feature = "proxy-tls")]
const SNI_PEEK_MAX_BYTES: usize = 16 * 1024;

/// What peeking at a connection's opening bytes established about its hostname.
///
/// The third case is the one that matters: "no hostname" and "hostname unknown"
/// must not be treated alike, because a connection whose hostname could not be
/// read may well belong to a passthrough daemon.
#[cfg(feature = "proxy-tls")]
#[derive(Debug, PartialEq, Eq)]
enum SniProbe {
    /// The ClientHello named this host.
    Host(String),
    /// A complete ClientHello carried no SNI, or the connection is not a TLS
    /// handshake at all. Either way it cannot name a passthrough daemon.
    NoHost,
    /// No verdict: the hello stalled, exceeded the inspection window, or never
    /// reconciled its own length fields. Passthrough cannot be ruled out.
    Undetermined,
}

/// Read the SNI hostname from a connection's ClientHello *without consuming
/// it*, so the same bytes are still available to whichever path handles the
/// connection.
///
/// `timeout` bounds the whole probe, including a read that never completes:
/// a peer that opens a connection and then sends nothing more is given up on
/// rather than holding the connection task open. A client that sends part of
/// a hello and then closes is noticed as soon as its close arrives, even
/// though `peek` keeps returning the buffered bytes rather than end of file,
/// and is reported as [`SniProbe::Undetermined`] without waiting out the
/// timeout.
#[cfg(feature = "proxy-tls")]
async fn peek_sni_host(stream: &TcpStream, timeout: std::time::Duration) -> SniProbe {
    use crate::proxy::sni::{SniPeek, parse_sni};

    const MIN_PAUSE: std::time::Duration = std::time::Duration::from_millis(10);
    const MAX_PAUSE: std::time::Duration = std::time::Duration::from_millis(200);

    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = vec![0u8; 2048];
    let mut last_n = 0;
    let mut pause = MIN_PAUSE;

    loop {
        // The deadline has to bound the read itself: a peer that opens a
        // connection and then stops sending leaves `peek` waiting forever, and
        // the check further down never runs to end it.
        let n = match tokio::time::timeout_at(deadline, stream.peek(&mut buf)).await {
            // End of file with nothing buffered: the client hung up before
            // saying anything, so there is nothing to route and nothing to
            // downgrade.
            Ok(Ok(0)) => return SniProbe::NoHost,
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                log::debug!("Failed to peek at a TLS connection: {e}");
                return SniProbe::Undetermined;
            }
            Err(_elapsed) => {
                log::debug!("Timed out waiting for a client that sent no ClientHello");
                return SniProbe::Undetermined;
            }
        };
        match parse_sni(&buf[..n]) {
            SniPeek::Found(host) => return SniProbe::Host(host),
            SniPeek::Absent | SniPeek::NotTls => return SniProbe::NoHost,
            SniPeek::Incomplete => {}
        }

        // The hello may simply be longer than the window we looked at.
        if n == buf.len() && buf.len() < SNI_PEEK_MAX_BYTES {
            buf.resize((buf.len() * 2).min(SNI_PEEK_MAX_BYTES), 0);
            continue;
        }
        if n >= SNI_PEEK_MAX_BYTES {
            log::debug!("Giving up on SNI after {n} bytes without a complete ClientHello");
            return SniProbe::Undetermined;
        }
        if tokio::time::Instant::now() >= deadline {
            log::debug!("Timed out waiting for a complete ClientHello ({n} bytes read)");
            return SniProbe::Undetermined;
        }
        // Peeked data stays in the socket buffer, so the socket always reads as
        // readable and waiting on that would spin. Its readiness does record
        // the peer's close, though, which `peek` cannot report while bytes are
        // still buffered: a client that sent part of a hello and hung up will
        // never finish it.
        match stream.ready(tokio::io::Interest::READABLE).await {
            Ok(ready) if ready.is_read_closed() => {
                log::debug!("Client closed after {n} bytes of an incomplete ClientHello");
                return SniProbe::Undetermined;
            }
            Ok(_) => {}
            Err(e) => {
                log::debug!("Failed to poll a TLS connection: {e}");
                return SniProbe::Undetermined;
            }
        }
        // Re-peek soon while the hello is still arriving, and back off while
        // it has stalled, so a slow client is not polled hundreds of times.
        pause = if n > last_n {
            MIN_PAUSE
        } else {
            (pause * 2).min(MAX_PAUSE)
        };
        last_n = n;
        tokio::time::sleep_until((tokio::time::Instant::now() + pause).min(deadline)).await;
    }
}

/// Splice a TLS connection straight through to its daemon.
///
/// Neither direction is inspected or rewritten, so the daemon terminates TLS
/// with its own certificate, negotiates its own ALPN (HTTP/2 and gRPC included)
/// and can require client certificates — none of which survive termination at
/// the proxy.
///
/// A stopped daemon is auto-started first and the connection is held until it
/// is ready, bounded by `proxy.auto_start_timeout`: a raw TLS stream has no
/// equivalent of the HTML "Starting…" page. When routing fails there is
/// likewise nothing to reply with, so the connection is closed and the reason
/// is logged.
#[cfg(feature = "proxy-tls")]
async fn serve_passthrough(mut stream: TcpStream, host: &str, tld: &str) {
    let port = match resolve_passthrough_port(host, tld).await {
        Ok(port) => port,
        Err(msg) => {
            log::warn!("TLS passthrough for '{host}' failed: {msg}");
            return;
        }
    };

    let addr = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let mut backend = match connect_backend(addr).await {
        Ok(b) => b,
        Err(e) => {
            log::warn!("TLS passthrough for '{host}': failed to connect to {addr}: {e}");
            return;
        }
    };

    log::debug!("TLS passthrough: splicing '{host}' to {addr}");
    if let Err(e) = tokio::io::copy_bidirectional(&mut stream, &mut backend).await {
        // A client or daemon closing one half mid-stream is ordinary.
        log::debug!("TLS passthrough for '{host}' ended: {e}");
    }
}

/// How long to keep retrying a refused connection to a daemon that has just
/// been reported ready.
///
/// A daemon can be recorded as running and holding a port a moment before its
/// listener actually accepts. This window covers that gap without making a
/// genuinely dead port hang the client for the full auto-start budget.
#[cfg(feature = "proxy-tls")]
const PASSTHROUGH_CONNECT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Connect to a daemon's port, retrying a refused connection briefly.
#[cfg(feature = "proxy-tls")]
async fn connect_backend(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let deadline = tokio::time::Instant::now() + PASSTHROUGH_CONNECT_GRACE;
    loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                let retryable = e.kind() == std::io::ErrorKind::ConnectionRefused;
                if !retryable || tokio::time::Instant::now() >= deadline {
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Resolve the daemon port a passthrough hostname splices to, starting the
/// daemon if it is not running.
///
/// Waiting replaces the "Starting…" page that the HTTP path shows, and the
/// whole resolution — waiting behind another connection's start *and* running
/// one of its own — is bounded by a single `proxy.auto_start_timeout` budget,
/// which is what the documentation promises.
#[cfg(feature = "proxy-tls")]
async fn resolve_passthrough_port(host: &str, tld: &str) -> std::result::Result<u16, String> {
    let budget = settings().proxy_auto_start_timeout();
    match tokio::time::timeout(budget, resolve_passthrough_port_inner(host, tld)).await {
        Ok(result) => result,
        Err(_elapsed) => Err(format!(
            "no daemon was ready for '{host}' within proxy.auto_start_timeout ({budget:?})"
        )),
    }
}

/// Inner loop of [`resolve_passthrough_port`], wrapped by the caller so that
/// waiting and starting share one deadline.
#[cfg(feature = "proxy-tls")]
async fn resolve_passthrough_port_inner(host: &str, tld: &str) -> std::result::Result<u16, String> {
    loop {
        match resolve_target(host, tld).await {
            ResolveResult::Ready(port) => return Ok(port),
            // Another connection is already auto-starting this daemon; wait
            // for it rather than starting a second copy. The caller's timeout
            // ends this wait.
            ResolveResult::Starting { slug: _ } => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            ResolveResult::NotFound => {
                return Err(
                    "no running daemon with a port matched this hostname, and it could not \
                     be auto-started"
                        .to_string(),
                );
            }
            // A project or stack page is HTML the proxy serves itself, and an
            // unknown name has a page listing what does exist. Neither is
            // something a spliced TLS stream can carry, so the connection is
            // closed with the reason logged.
            ResolveResult::Page {
                project, worktree, ..
            } => {
                return Err(match worktree {
                    Some(worktree) => {
                        format!("'{worktree}' of project '{project}' is a stack page, not a daemon")
                    }
                    None => format!("'{project}' is a project page, not a daemon"),
                });
            }
            ResolveResult::Unknown { heading, .. } => return Err(heading),
            ResolveResult::Error(msg) => return Err(msg),
        }
    }
}

/// Fallback when proxy-tls feature is not enabled.
#[cfg(not(feature = "proxy-tls"))]
#[allow(clippy::too_many_arguments)]
async fn serve_https_with_http_fallback(
    _app: Router,
    _addr: SocketAddr,
    _s: &crate::settings::Settings,
    _effective_port: u16,
    _effective_tld: String,
    _plain_proxy_state: ProxyState,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    _cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let msg = "HTTPS proxy support requires the `proxy-tls` feature.\n\
         Rebuild pitchfork with: cargo build --features proxy-tls"
        .to_string();
    let _ = bind_tx.send(Err(msg.clone()));
    miette::bail!("{msg}")
}

/// Resolve the CA certificate and key paths from settings.
///
/// If `tls_cert` / `tls_key` are empty, falls back to the auto-generated
/// CA paths in `$PITCHFORK_STATE_DIR/proxy/`.
///
/// Setting only one of the two is refused. Filling the other half from the
/// generated CA would pair a user's key with pitchfork's certificate, or the
/// reverse, and when the CA does not exist yet, generating it would write a
/// new CA key over the user's file.
#[cfg(feature = "proxy-tls")]
fn resolve_tls_paths(
    s: &crate::settings::Settings,
) -> crate::Result<(std::path::PathBuf, std::path::PathBuf)> {
    if let Some(problem) = tls_pair_problem(&s.proxy.tls_cert, &s.proxy.tls_key) {
        miette::bail!("{problem}");
    }
    let proxy_dir = crate::env::PITCHFORK_STATE_DIR.join("proxy");
    let resolve = |configured: &str, default: &str| {
        if configured.is_empty() {
            proxy_dir.join(default)
        } else {
            std::path::PathBuf::from(configured)
        }
    };
    Ok((
        resolve(&s.proxy.tls_cert, "ca.pem"),
        resolve(&s.proxy.tls_key, "ca-key.pem"),
    ))
}

/// Why `proxy.tls_cert` and `proxy.tls_key` cannot be used as configured:
/// they are a pair, so both are set or neither is.
pub(crate) fn tls_pair_problem(cert: &str, key: &str) -> Option<String> {
    match (cert.is_empty(), key.is_empty()) {
        (false, true) => Some(
            "proxy.tls_cert is set but proxy.tls_key is empty; set both, or neither to use \
             the generated CA"
                .to_string(),
        ),
        (true, false) => Some(
            "proxy.tls_key is set but proxy.tls_cert is empty; set both, or neither to use \
             the generated CA"
                .to_string(),
        ),
        _ => None,
    }
}

/// Generate the CA pair unless `usable` says the one on disk will do.
///
/// `proxy setup` and a starting supervisor can both find the CA missing and
/// generate one at once. The cert and key are separate files, so two writers
/// interleaving can leave one's certificate beside the other's key: a pair
/// that trusts fine and then signs nothing that verifies. The check and the
/// write happen under one lock so only the first generation happens.
///
/// Returns whether a new pair was written.
#[cfg(feature = "proxy-tls")]
pub fn ensure_ca(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    usable: impl FnOnce() -> bool,
) -> crate::Result<bool> {
    let _lock = xx::fslock::get(cert_path, false)
        .map_err(|e| miette::miette!("Failed to lock {}: {e}", cert_path.display()))?;
    if usable() {
        return Ok(false);
    }
    generate_ca(cert_path, key_path)?;
    Ok(true)
}

/// Generate a local root CA certificate and private key using `rcgen`.
///
/// The CA is used to sign per-domain certificates on demand (SNI).
/// Files are written in PEM format to `cert_path` and `key_path`. Callers go
/// through [`ensure_ca`], which holds the CA lock around the check and this
/// write.
#[cfg(feature = "proxy-tls")]
fn generate_ca(cert_path: &std::path::Path, key_path: &std::path::Path) -> crate::Result<()> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyUsagePurpose,
    };

    // Create parent directory if needed
    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| miette::miette!("Failed to create proxy cert directory: {e}"))?;
    }

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Pitchfork Local CA");
    dn.push(DnType::OrganizationName, "Pitchfork");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| miette::miette!("Failed to generate CA key pair: {e}"))?;
    let ca_cert = params
        .self_signed(&key_pair)
        .map_err(|e| miette::miette!("Failed to self-sign CA certificate: {e}"))?;

    // Write the CA certificate (public — 0644 is fine)
    std::fs::write(cert_path, ca_cert.pem()).map_err(|e| {
        miette::miette!(
            "Failed to write CA certificate to {}: {e}",
            cert_path.display()
        )
    })?;

    // Write the CA private key with restrictive permissions (0600).
    // Using OpenOptions + mode() so the file is never world-readable,
    // even briefly before a chmod call.
    {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(key_path)
                .and_then(|mut f| f.write_all(key_pair.serialize_pem().as_bytes()))
                .map_err(|e| {
                    miette::miette!("Failed to write CA key to {}: {e}", key_path.display())
                })?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(key_path, key_pair.serialize_pem()).map_err(|e| {
                miette::miette!("Failed to write CA key to {}: {e}", key_path.display())
            })?;
            log::debug!(
                "CA private key written to {} (file permissions are not restricted \
                 on non-Unix platforms — consider restricting access manually)",
                key_path.display()
            );
        }
    }

    Ok(())
}

/// Resolver that serves one configured certificate for every connection.
///
/// Used when `proxy.tls_cert` / `proxy.tls_key` are set. No certificate is
/// minted: whatever the user configured is what clients see, for every SNI
/// name.
#[cfg(feature = "proxy-tls")]
struct StaticCertResolver {
    certified: Arc<rustls::sign::CertifiedKey>,
    /// TLD hostnames are resolved against, so a passthrough hostname is
    /// refused here too rather than answered with the configured certificate.
    tld: String,
}

#[cfg(feature = "proxy-tls")]
impl std::fmt::Debug for StaticCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticCertResolver").finish_non_exhaustive()
    }
}

#[cfg(feature = "proxy-tls")]
impl StaticCertResolver {
    fn new(
        cert_path: &std::path::Path,
        key_path: &std::path::Path,
        tld: String,
    ) -> crate::Result<Self> {
        use rustls::pki_types::CertificateDer;
        use rustls_pemfile::{certs, private_key};

        let cert_pem = std::fs::read(cert_path).map_err(|e| {
            miette::miette!("Failed to read proxy.tls_cert {}: {e}", cert_path.display())
        })?;
        let key_pem = std::fs::read(key_path).map_err(|e| {
            miette::miette!("Failed to read proxy.tls_key {}: {e}", key_path.display())
        })?;

        let cert_ders: Vec<CertificateDer<'static>> = certs(&mut cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| miette::miette!("Failed to parse {}: {e}", cert_path.display()))?;
        if cert_ders.is_empty() {
            miette::bail!("No certificates found in {}", cert_path.display());
        }

        let key_der = private_key(&mut key_pem.as_slice())
            .map_err(|e| miette::miette!("Failed to parse {}: {e}", key_path.display()))?
            .ok_or_else(|| miette::miette!("No private key found in {}", key_path.display()))?;
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| miette::miette!("Failed to use the configured private key: {e}"))?;

        let certified = rustls::sign::CertifiedKey::new(cert_ders, signing_key);
        // `CertifiedKey::new` only packages the two; it does not check that the
        // key belongs to the certificate. Without this the listener would start
        // cleanly and then fail every single handshake.
        certified.keys_match().map_err(|e| {
            miette::miette!(
                "proxy.tls_key {} does not match proxy.tls_cert {}: {e}",
                key_path.display(),
                cert_path.display()
            )
        })?;

        Ok(Self {
            certified: Arc::new(certified),
            tld,
        })
    }
}

#[cfg(feature = "proxy-tls")]
impl rustls::server::ResolvesServerCert for StaticCertResolver {
    fn resolve(
        &self,
        client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        // As in `SniCertResolver::resolve`: a passthrough hostname whose hello
        // could not be read before the handshake must fail rather than be
        // answered with a certificate that is not the daemon's.
        if let Some(domain) = client_hello.server_name()
            && resolve_tls_mode_in(domain, &self.tld, &slug_snapshot(), &registry_snapshot())
                .is_passthrough()
        {
            log::warn!(
                "Refusing to terminate TLS for '{domain}', which is configured for \
                 proxy_tls = \"passthrough\": its ClientHello could not be inspected before the \
                 handshake, so the stream could not be spliced to the daemon."
            );
            return None;
        }
        Some(Arc::clone(&self.certified))
    }
}

/// SNI-based certificate resolver.
///
/// Holds the local CA and a two-level cache of per-domain certificates:
/// - L1: in-memory `HashMap` (fastest, process-lifetime)
/// - L2: on-disk `host-certs/<safe_name>.pem` (survives restarts)
///
/// A `pending` set prevents concurrent requests for the same domain from
/// triggering multiple simultaneous cert-generation operations.
///
/// On each new TLS connection, `resolve()` is called with the SNI hostname;
/// if no cached cert exists for that domain, one is signed by the CA on the fly.
///
/// # Locking strategy
/// Both `cache` and `pending` use `std::sync::Mutex` paired with a
/// `std::sync::Condvar`.  The critical sections are intentionally short
/// (hash-map lookups / inserts), so the blocking time is negligible.
/// `get_or_create` is only called from the synchronous `ResolvesServerCert`
/// trait method (not from an async context), so blocking a thread here is
/// acceptable.
#[cfg(feature = "proxy-tls")]
struct SniCertResolver {
    /// The CA issuer (key + parsed cert params, used to sign leaf certs).
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
    /// TLD this CA is willing to sign for; anything else is refused. Also the
    /// TLD hostnames are resolved against, so a passthrough hostname can be
    /// recognized before a certificate is issued for it.
    tld: String,
    /// Directory where per-domain PEM files are cached on disk.
    host_certs_dir: std::path::PathBuf,
    /// L1 cache: domain → certified key (in-memory), with insertion order kept
    /// alongside so the oldest can be evicted once the cache is full.
    cache: std::sync::Mutex<CertCache>,
    /// Pending set: domains currently being generated (dedup concurrent requests).
    /// Using a `Condvar` so waiting threads are parked instead of spin-sleeping,
    /// which avoids blocking tokio worker threads.
    pending: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Condvar paired with `pending` — notified when a domain is removed from the set.
    pending_cv: std::sync::Condvar,
}

/// Delete the oldest cached certificates until at most [`MAX_HOST_CERTS`] remain.
///
/// Called at startup, because eviction during a run only knows about names that
/// run has seen. Oldest is by modification time, which is when the certificate
/// was minted, so the ones most recently useful survive.
#[cfg(feature = "proxy-tls")]
fn prune_host_certs(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "pem"))
        .filter_map(|e| {
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, e.path()))
        })
        .collect();
    if files.len() <= MAX_HOST_CERTS {
        return;
    }
    files.sort_by_key(|(t, _)| *t);
    let excess = files.len() - MAX_HOST_CERTS;
    for (_, path) in files.into_iter().take(excess) {
        if let Err(e) = std::fs::remove_file(&path) {
            log::debug!("Could not prune cached cert {}: {e}", path.display());
        }
    }
}

/// Minted certificates, bounded and evicted oldest-first.
#[cfg(feature = "proxy-tls")]
#[derive(Default)]
struct CertCache {
    by_domain: std::collections::HashMap<String, Arc<rustls::sign::CertifiedKey>>,
    /// Insertion order, oldest first.
    order: std::collections::VecDeque<String>,
}

#[cfg(feature = "proxy-tls")]
impl CertCache {
    fn get(&self, domain: &str) -> Option<&Arc<rustls::sign::CertifiedKey>> {
        self.by_domain.get(domain)
    }

    /// Insert `key`, evicting the oldest entry when full.
    ///
    /// Returns the evicted domain, whose on-disk copy the caller removes.
    fn insert(&mut self, domain: String, key: Arc<rustls::sign::CertifiedKey>) -> Option<String> {
        if self.by_domain.insert(domain.clone(), key).is_none() {
            self.order.push_back(domain);
        }
        if self.by_domain.len() <= MAX_HOST_CERTS {
            return None;
        }
        let evicted = self.order.pop_front()?;
        self.by_domain.remove(&evicted);
        Some(evicted)
    }
}

#[cfg(feature = "proxy-tls")]
impl std::fmt::Debug for SniCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SniCertResolver").finish_non_exhaustive()
    }
}

#[cfg(feature = "proxy-tls")]
impl SniCertResolver {
    /// Load the CA from disk and prepare the resolver.
    fn new(
        ca_cert_path: &std::path::Path,
        ca_key_path: &std::path::Path,
        tld: String,
    ) -> crate::Result<Self> {
        let ca_key_pem = std::fs::read_to_string(ca_key_path)
            .map_err(|e| miette::miette!("Failed to read CA key {}: {e}", ca_key_path.display()))?;
        let ca_cert_pem = std::fs::read_to_string(ca_cert_path).map_err(|e| {
            miette::miette!("Failed to read CA cert {}: {e}", ca_cert_path.display())
        })?;

        // Verify the PEM is readable (sanity check)
        if !ca_cert_pem.contains("BEGIN CERTIFICATE") {
            miette::bail!("CA cert file does not contain a valid PEM certificate");
        }

        let ca_key = rcgen::KeyPair::from_pem(&ca_key_pem)
            .map_err(|e| miette::miette!("Failed to parse CA key: {e}"))?;

        // Parse the CA cert + key into an Issuer for signing leaf certs.
        let issuer = rcgen::Issuer::from_ca_cert_pem(&ca_cert_pem, ca_key)
            .map_err(|e| miette::miette!("Failed to parse CA cert: {e}"))?;

        // Ensure the host-certs directory exists
        let host_certs_dir = ca_cert_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("host-certs");
        std::fs::create_dir_all(&host_certs_dir)
            .map_err(|e| miette::miette!("Failed to create host-certs dir: {e}"))?;
        // The in-memory cache starts empty on every run, so without this the
        // directory would keep files from previous processes for ever and grow
        // across restarts however well eviction works within one.
        prune_host_certs(&host_certs_dir);

        Ok(Self {
            issuer,
            tld,
            host_certs_dir,
            cache: std::sync::Mutex::new(CertCache::default()),
            pending: std::sync::Mutex::new(std::collections::HashSet::new()),
            pending_cv: std::sync::Condvar::new(),
        })
    }

    /// `get_or_create`, but only for names this CA is allowed to sign.
    ///
    /// The CA is installed in the machine's trust store and the proxy answers on
    /// 127.0.0.1, so without this check any name that resolves to loopback — an
    /// /etc/hosts line, a rebinding answer, a poisoned resolver — could obtain a
    /// browser-trusted certificate for itself and its parent wildcard just by
    /// sending it as SNI. Returning `None` fails the handshake and writes
    /// nothing to the on-disk cache.
    fn get_or_create_checked(&self, domain: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        if !crate::proxy::owns_name(&self.tld, domain) {
            // Throttled for the same reason the refusal exists: anything that
            // can reach the listener can ask for any name, repeatedly.
            if let Some(suppressed) = REFUSED_SNI.allow(REFUSAL_LOG_INTERVAL) {
                log::warn!(
                    "Refusing to issue a certificate for {domain:?}: \
                     the pitchfork CA only signs names under .{} \
                     ({suppressed} similar refusals since the last message)",
                    self.tld
                );
            }
            return None;
        }
        self.get_or_create(domain)
    }

    /// Get or create a `CertifiedKey` for the given domain.
    ///
    /// Resolution order:
    /// 1. L1 in-memory cache
    /// 2. L2 on-disk cache (`host-certs/<safe_name>.pem`)
    /// 3. Generate fresh cert, persist to disk, populate both caches
    ///
    /// Concurrent requests for the same domain are deduplicated: the second
    /// thread waits on a `Condvar` until the first thread finishes, then reads
    /// from the cache.  This avoids both duplicate cert generation and the
    /// spin-sleep anti-pattern that would block tokio worker threads.
    ///
    /// # Locking discipline
    /// `cache` and `pending` are **never held simultaneously**.  The protocol is:
    /// 1. Check `cache` (lock, read, unlock).
    /// 2. Acquire `pending`; wait if domain is in-progress; re-check `cache`
    ///    after waking (unlock `cache` before re-acquiring `pending` is not
    ///    needed because we release `cache` before entering the `pending` block).
    /// 3. Insert domain into `pending`; release `pending` lock.
    /// 4. Generate cert (no locks held).
    /// 5. Insert into `cache` (lock, write, unlock).
    /// 6. Remove from `pending` and notify (lock, write, unlock).
    fn get_or_create(&self, domain: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        // L1: memory cache (fast path — no pending lock needed)
        {
            let cache = self.cache.lock().ok()?;
            if let Some(ck) = cache.get(domain) {
                return Some(Arc::clone(ck));
            }
        } // cache lock released here

        // Dedup: acquire the pending lock, wait if another thread is generating
        // this domain, then re-check the cache (without holding pending) before
        // deciding to generate.
        //
        // We deliberately release the pending lock before re-checking the cache
        // to avoid holding both locks simultaneously.  The re-check is safe
        // because: if the generating thread inserted into the cache and then
        // removed from pending, we will see the cert in the cache.  If we miss
        // the window (extremely unlikely), we will generate a duplicate cert,
        // which is harmless — the last writer wins in the cache.
        loop {
            {
                let mut pending = self.pending.lock().ok()?;
                if pending.contains(domain) {
                    // Another thread is generating; wait until it finishes.
                    pending = self.pending_cv.wait(pending).ok()?;
                    // pending lock re-acquired; loop to re-check cache below.
                    drop(pending);
                } else {
                    // No one else is generating; claim the slot and proceed.
                    pending.insert(domain.to_string());
                    break;
                }
            } // pending lock released

            // Re-check cache after being woken (the generating thread may have
            // already populated it).  Cache lock is acquired independently of
            // pending lock here — no nesting.
            {
                let cache = self.cache.lock().ok()?;
                if let Some(ck) = cache.get(domain) {
                    return Some(Arc::clone(ck));
                }
            } // cache lock released
        } // pending lock released at break

        let result = self.get_or_create_inner(domain);

        // Always clear the pending flag and wake waiting threads.
        // notify_all() is called *inside* the lock scope so that the domain is
        // guaranteed to be removed before any waiting thread is woken up.
        // If the lock is poisoned we recover it (the data is still valid) so
        // that the domain is always removed and waiters are always notified.
        {
            let mut pending = match self.pending.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            pending.remove(domain);
            self.pending_cv.notify_all();
        }

        result
    }

    /// Inner implementation: check disk cache, then generate.
    fn get_or_create_inner(&self, domain: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let disk_path = self.disk_path(domain);

        // L2: disk cache — try to load existing cert+key PEM
        if disk_path.exists() {
            if let Ok(ck) = self.load_from_disk(&disk_path) {
                let ck = Arc::new(ck);
                self.remember(domain, &ck);
                return Some(ck);
            }
            // Disk cache corrupt/expired — fall through to regenerate
            let _ = std::fs::remove_file(&disk_path);
        }

        // L3: generate fresh cert
        let ck = self.sign_for_domain(domain).ok()?;

        let ck = Arc::new(ck);
        self.remember(domain, &ck);
        Some(ck)
    }

    /// Cache `ck` for `domain`, dropping the oldest entry when full.
    ///
    /// The evicted certificate's file is removed too, so the on-disk cache
    /// stays bounded alongside the in-memory one.
    fn remember(&self, domain: &str, ck: &Arc<rustls::sign::CertifiedKey>) {
        let evicted = match self.cache.lock() {
            Ok(mut cache) => cache.insert(domain.to_string(), Arc::clone(ck)),
            Err(_) => return,
        };
        if let Some(evicted) = evicted {
            let path = self.disk_path(&evicted);
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                log::debug!("Could not evict cached cert {}: {e}", path.display());
            }
        }
    }

    /// Where the cached certificate for `domain` lives.
    fn disk_path(&self, domain: &str) -> std::path::PathBuf {
        self.host_certs_dir
            .join(format!("{}.pem", cert_cache_file_stem(domain)))
    }

    /// Load a `CertifiedKey` from a combined cert+key PEM file on disk.
    ///
    /// Returns an error if the certificate has already expired, so the caller
    /// can fall through to regeneration rather than serving a stale cert.
    fn load_from_disk(&self, path: &std::path::Path) -> crate::Result<rustls::sign::CertifiedKey> {
        use rustls::pki_types::CertificateDer;
        use rustls_pemfile::{certs, private_key};

        let pem = std::fs::read_to_string(path)
            .map_err(|e| miette::miette!("Failed to read disk cert {}: {e}", path.display()))?;

        let cert_ders: Vec<CertificateDer<'static>> = certs(&mut pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| miette::miette!("Failed to parse certs from {}: {e}", path.display()))?;

        if cert_ders.is_empty() {
            miette::bail!("No certificates found in {}", path.display());
        }

        // Check that the first certificate has not expired using x509-parser.
        {
            let (_, cert) = x509_parser::parse_x509_certificate(&cert_ders[0]).map_err(|e| {
                miette::miette!("Failed to parse certificate from {}: {e}", path.display())
            })?;
            use chrono::Utc;
            let now_ts = Utc::now().timestamp();
            let not_after_ts = cert.validity().not_after.timestamp();
            if not_after_ts < now_ts {
                miette::bail!(
                    "Cached certificate at {} has expired — will regenerate",
                    path.display()
                );
            }
        }

        let key_der = private_key(&mut pem.as_bytes())
            .map_err(|e| miette::miette!("Failed to parse key from {}: {e}", path.display()))?
            .ok_or_else(|| miette::miette!("No private key found in {}", path.display()))?;

        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| miette::miette!("Failed to create signing key from disk: {e}"))?;

        Ok(rustls::sign::CertifiedKey::new(cert_ders, signing_key))
    }

    /// Sign a leaf certificate for `domain` using the CA.
    ///
    /// SANs include:
    /// - `DNS:<domain>` (exact match)
    /// - `DNS:*.<parent>` (sibling wildcard, e.g. `*.pf.localhost` for `docs.pf.localhost`)
    ///
    /// Returns both the `CertifiedKey` and the combined PEM for disk caching.
    fn sign_for_domain(&self, domain: &str) -> crate::Result<rustls::sign::CertifiedKey> {
        use rcgen::date_time_ymd;
        use rcgen::{CertificateParams, DistinguishedName, DnType, SanType};
        use rustls::pki_types::CertificateDer;
        use rustls_pemfile::private_key;

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, domain);
        params.distinguished_name = dn;

        // Set validity dynamically: from yesterday to 10 years from now.
        {
            use chrono::{Datelike, Duration, Utc};
            let yesterday = Utc::now() - Duration::days(1);
            // 397 days: stays within Chrome/Safari's 398-day maximum validity limit
            // for TLS certificates (including locally-trusted CA leaf certs).
            let expiry = Utc::now() + Duration::days(397);
            params.not_before = date_time_ymd(
                yesterday.year(),
                yesterday.month() as u8,
                yesterday.day() as u8,
            );
            params.not_after =
                date_time_ymd(expiry.year(), expiry.month() as u8, expiry.day() as u8);
        }

        // Build SANs: exact domain + sibling wildcard (e.g. *.pf.localhost)
        let mut sans =
            vec![SanType::DnsName(domain.to_string().try_into().map_err(
                |e| miette::miette!("Invalid domain name '{domain}': {e}"),
            )?)];
        // Sibling wildcard for the parent domain, one level up.
        //
        // Only when the parent is strictly inside the TLD. `*.<tld>` would cover
        // every name the proxy serves, which is far broader than the one host
        // this certificate is for, and a parent outside the TLD is not ours to
        // claim at all.
        if let Some(dot_pos) = domain.find('.') {
            let parent = &domain[dot_pos + 1..];
            if crate::proxy::is_strictly_under_tld(&self.tld, parent) {
                let wildcard = format!("*.{parent}");
                if let Ok(wc) = wildcard.try_into() {
                    sans.push(SanType::DnsName(wc));
                }
            }
        }
        params.subject_alt_names = sans;

        let leaf_key = rcgen::KeyPair::generate()
            .map_err(|e| miette::miette!("Failed to generate leaf key: {e}"))?;
        let leaf_cert = params
            .signed_by(&leaf_key, &self.issuer)
            .map_err(|e| miette::miette!("Failed to sign leaf cert for '{domain}': {e}"))?;

        // Convert to rustls types
        let cert_der = CertificateDer::from(leaf_cert.der().to_vec());
        let key_pem = leaf_key.serialize_pem();
        let key_der = private_key(&mut key_pem.as_bytes())
            .map_err(|e| miette::miette!("Failed to parse leaf key PEM: {e}"))?
            .ok_or_else(|| miette::miette!("No private key found in generated PEM"))?;

        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| miette::miette!("Failed to create signing key: {e}"))?;

        // Persist cert + key to disk cache as combined PEM.
        // Use 0600 so the private key is not world-readable.
        let disk_path = self.disk_path(domain);
        let combined_pem = format!("{}{}", leaf_cert.pem(), key_pem);
        {
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                if let Err(e) = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&disk_path)
                    .and_then(|mut f| f.write_all(combined_pem.as_bytes()))
                {
                    log::warn!(
                        "Failed to persist cert for '{domain}' to {}: {e}",
                        disk_path.display()
                    );
                }
            }
            #[cfg(not(unix))]
            {
                if let Err(e) = std::fs::write(&disk_path, combined_pem) {
                    log::warn!(
                        "Failed to persist cert for '{domain}' to {}: {e}",
                        disk_path.display()
                    );
                } else {
                    log::debug!(
                        "Leaf cert for '{domain}' written to {} (file permissions are not \
                         restricted on non-Unix platforms — consider restricting access manually)",
                        disk_path.display()
                    );
                }
            }
        }

        Ok(rustls::sign::CertifiedKey::new(vec![cert_der], signing_key))
    }
}

#[cfg(feature = "proxy-tls")]
impl rustls::server::ResolvesServerCert for SniCertResolver {
    fn resolve(
        &self,
        client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let domain = client_hello.server_name()?;
        // This is the last point where a passthrough hostname can still be
        // caught, and the first where the host name is authoritative: rustls
        // has reassembled the handshake itself. Issuing here would answer with
        // the proxy's certificate in place of the daemon's and drop the
        // client-certificate request, so the handshake is failed instead.
        // Reached only when the pre-handshake peek could not read the hello.
        if resolve_tls_mode_in(domain, &self.tld, &slug_snapshot(), &registry_snapshot())
            .is_passthrough()
        {
            log::warn!(
                "Refusing to terminate TLS for '{domain}', which is configured for \
                 proxy_tls = \"passthrough\": its ClientHello could not be inspected before the \
                 handshake, so the stream could not be spliced to the daemon."
            );
            return None;
        }

        // Refuse to sign for a name outside the proxy's own TLD.
        //
        // The CA is installed in the machine's trust store and the proxy answers
        // on 127.0.0.1, so without this check any name that resolves to loopback
        // — an /etc/hosts line, a rebinding answer, a poisoned resolver — could
        // obtain a browser-trusted certificate for itself and its parent
        // wildcard just by sending it as SNI. Returning `None` fails the
        // handshake and writes nothing to the on-disk cache.
        self.get_or_create_checked(domain)
    }
}

/// Why the CA pair at `cert` and `key` cannot sign certificates, if it cannot.
///
/// Existence is not enough: a truncated file or a key that belongs to another
/// certificate stops the HTTPS listener from starting, and trusting such a
/// certificate would install something the proxy never serves from.
#[cfg(feature = "proxy-tls")]
pub(crate) fn ca_pair_problem(cert: &std::path::Path, key: &std::path::Path) -> Option<String> {
    use rcgen::PublicKeyData;

    let cert_pem = match std::fs::read_to_string(cert) {
        Ok(p) => p,
        Err(e) => return Some(format!("cannot read {}: {e}", cert.display())),
    };
    let key_pem = match std::fs::read_to_string(key) {
        Ok(p) => p,
        Err(e) => return Some(format!("cannot read {}: {e}", key.display())),
    };
    let key_pair = match rcgen::KeyPair::from_pem(&key_pem) {
        Ok(k) => k,
        Err(e) => return Some(format!("cannot parse {}: {e}", key.display())),
    };
    let Some(Ok(der)) = rustls_pemfile::certs(&mut cert_pem.as_bytes()).next() else {
        return Some(format!("no certificate in {}", cert.display()));
    };
    let parsed = match x509_parser::parse_x509_certificate(&der) {
        Ok((_, c)) => c,
        Err(e) => return Some(format!("cannot parse {}: {e}", cert.display())),
    };
    if parsed.public_key().subject_public_key.data.as_ref() != key_pair.der_bytes() {
        return Some(format!(
            "{} is not the key for {}",
            key.display(),
            cert.display()
        ));
    }
    if let Err(e) = rcgen::Issuer::from_ca_cert_pem(&cert_pem, key_pair) {
        return Some(format!("{} cannot sign certificates: {e}", cert.display()));
    }
    None
}

/// A file name for `domain`'s cached certificate, distinct for distinct names.
///
/// Letters, digits, `-` and `.` are kept, since they are safe in a file name
/// and make the cache readable; every other byte is percent-encoded. Mapping
/// `.` to `_`, as this once did, sent `a_b.localhost` and `a.b.localhost` to
/// the same file, so one name's certificate could be served for the other.
#[cfg(feature = "proxy-tls")]
fn cert_cache_file_stem(domain: &str) -> String {
    let mut out = String::with_capacity(domain.len());
    for b in domain.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'.' {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// State for the plain-HTTP side of the TLS listener.
#[derive(Clone)]
struct PlainState {
    tld: String,
    port: u16,
    /// Carried so `CONNECT` can be tunnelled without re-deriving the routing
    /// configuration.
    proxy: ProxyState,
}

/// Plain HTTP arriving on the HTTPS port.
///
/// `CONNECT` is tunnelled, because that is how a PAC-configured browser opens
/// an `https://` URL. Everything else is redirected to HTTPS.
async fn plain_fallback_handler(State(state): State<PlainState>, req: Request) -> Response {
    if req.method() == axum::http::Method::CONNECT {
        let raw_host = get_request_host(&req).unwrap_or_default();
        return connect_handler(&state.proxy, req, &raw_host).await;
    }
    redirect_to_https_handler(req).await
}

/// Render the PAC script, or an error page explaining why it could not be.
/// Refuse a write method aimed at the PAC file.
///
/// A PAC file is fetched, never written, so anything but `GET` or `HEAD` is a
/// mistake. Returning `Some` here rather than restricting the route keeps the
/// path available to proxied hostnames under every method.
fn reject_non_read(method: &axum::http::Method) -> Option<Response> {
    if matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD) {
        return None;
    }
    let mut res = error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "the proxy auto-config file is read-only\n",
    );
    res.headers_mut().insert(
        axum::http::header::ALLOW,
        HeaderValue::from_static("GET, HEAD"),
    );
    Some(res)
}

fn pac_response(tld: &str, host: &str, port: u16) -> Response {
    match crate::proxy::pac::generate(tld, host, port) {
        Ok(body) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/x-ns-proxy-autoconfig",
            )],
            body,
        )
            .into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Serve `/proxy.pac` on the proxy listener.
///
/// A request addressed to a name *beneath* the TLD is a normal proxied request
/// that happens to use this path, so it is forwarded to the backend instead.
/// The TLD apex is not: no slug can route it, so `http://localhost/proxy.pac`
/// with `proxy.tld = "localhost"` is a request for the PAC file.
async fn pac_handler(State(state): State<ProxyState>, req: Request) -> Response {
    let host = get_request_host(&req).unwrap_or_default();
    let bare = host.split(':').next().unwrap_or("");
    if !bare.is_empty() && crate::proxy::is_strictly_under_tld(&state.tld, bare) {
        return proxy_handler(State(state), req).await;
    }
    // Only now is this known to be a request for the PAC file itself, so the
    // method check belongs here rather than on the route.
    if let Some(deny) = reject_non_read(req.method()) {
        return deny;
    }
    let port = req
        .uri()
        .authority()
        .and_then(|a| a.port_u16())
        .or_else(|| host.rsplit(':').next().and_then(|p| p.parse().ok()))
        .unwrap_or(if state.is_tls { 443 } else { 80 });
    pac_response(&state.tld, &url_host(state.contact_ip), port)
}

/// Serve `/proxy.pac` over plain HTTP on the HTTPS listener.
async fn plain_pac_handler(State(state): State<PlainState>, req: Request) -> Response {
    // Same rule as the TLS side: a name beneath the TLD is a proxied request
    // that happens to use this path, so it is redirected to HTTPS like any
    // other. The apex is not routable, so it gets the PAC file.
    let host = get_request_host(&req).unwrap_or_default();
    let bare = host.split(':').next().unwrap_or("");
    if !bare.is_empty() && crate::proxy::is_strictly_under_tld(&state.tld, bare) {
        return redirect_to_https_handler(req).await;
    }
    if let Some(deny) = reject_non_read(req.method()) {
        return deny;
    }
    pac_response(&state.tld, &url_host(state.proxy.contact_ip), state.port)
}

/// The address a client on this machine uses to reach a listener bound to
/// `bind_ip`.
///
/// A wildcard bind is reachable over the loopback address of its own family; a
/// specific address is reachable at exactly that address.
fn local_contact_ip(bind_ip: std::net::IpAddr) -> std::net::IpAddr {
    match bind_ip {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        }
        ip => ip,
    }
}

/// Format an address for a URL or PAC directive, bracketing IPv6 literals.
fn url_host(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        std::net::IpAddr::V4(ip) => ip.to_string(),
    }
}

/// The port in a CONNECT authority (`host:port`, `[v6]:port`), if it names one.
fn connect_port(authority: &str) -> Option<u16> {
    let (host, port) = authority.rsplit_once(':')?;
    // A bare IPv6 literal's last colon is inside the address, not before a port.
    if host.contains(':') && !host.ends_with(']') {
        return None;
    }
    port.parse().ok()
}

/// Handle a `CONNECT` request from a client using the PAC file.
///
/// A proxy auto-config file routes `*.<tld>` through this listener, and a
/// browser opening an `https://` URL asks the proxy to tunnel with `CONNECT
/// host:port`. Without this the PAC path would only ever work for plain HTTP.
///
/// The tunnel's far end is this same listener: the target host is a name only
/// pitchfork resolves, and the TLS handshake inside the tunnel carries the SNI
/// the certificate resolver needs in order to mint a certificate for it. So the
/// connection is spliced back to the proxy's own address, where it arrives as an
/// ordinary TLS connection and is routed by `Host` as usual.
///
/// Only names under the configured TLD are tunnelled. An open CONNECT proxy
/// would let anything on the machine reach any host through pitchfork.
async fn connect_handler(state: &ProxyState, req: Request, raw_host: &str) -> Response {
    let authority = req
        .uri()
        .authority()
        .map(|a| a.as_str().to_string())
        .unwrap_or_else(|| raw_host.to_string());
    let host = authority
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(&authority)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();

    // The shared rule rather than a hand-rolled pair of conditions; keeping a
    // second copy of "is this name ours" is how several of these drifted apart.
    if !crate::proxy::owns_name(&state.tld, &host) {
        return error_response(
            StatusCode::FORBIDDEN,
            &format!(
                "pitchfork only tunnels CONNECT for names under .{} — refusing {host}",
                state.tld
            ),
        );
    }

    // With `proxy.https = false` the tunnel lands on a plain HTTP listener.
    // That still carries `ws://` and `http://` traffic, which browsers send
    // through CONNECT too, but a TLS handshake for an `https://` URL could
    // only fail there, and with an error that names neither cause nor fix.
    if !state.is_tls && connect_port(&authority) == Some(443) {
        return error_response(
            StatusCode::BAD_GATEWAY,
            &format!(
                "proxy.https is false, so pitchfork cannot serve https://{host}; \
                 use http:// or enable proxy.https"
            ),
        );
    }

    let Some(target) = state.connect_target else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "The proxy listener address is unknown, so CONNECT cannot be tunnelled",
        );
    };

    // Refuse once shutdown has begun, so no tunnel is created after the drain
    // has started looking for stragglers.
    if state.cancel.is_cancelled() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "The proxy is shutting down",
        );
    }

    // Bounded like the handshake phase and the DNS listener. The permit is
    // held for the life of the tunnel, which is the resource being limited.
    let Ok(permit) = Arc::clone(&state.tunnel_slots).try_acquire_owned() else {
        if let Some(suppressed) = REFUSED_TUNNEL.allow(REFUSAL_LOG_INTERVAL) {
            log::warn!(
                "Proxy refused a CONNECT tunnel: {MAX_TUNNELS} already open \
                 ({suppressed} similar refusals since the last message)"
            );
        }
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many CONNECT tunnels are open",
        );
    };
    let cancel = state.cancel.clone();

    // Reply 200 first, then take over the upgraded stream. hyper only hands the
    // socket over once the response has been sent.
    //
    // Tracked so shutdown can wait for the splice to stop rather than leaving
    // it holding two sockets.
    state.tunnels.spawn(async move {
        // Dropped with the task, releasing the slot when the tunnel closes.
        let _permit = permit;

        // Both of these happen before there is a tunnel to speak of, and
        // neither is bounded by anything on its own. A client that never
        // finishes reading the 200 leaves `upgrade::on` pending forever, and a
        // target that accepts nothing leaves `connect` the same way — in each
        // case holding one of the tunnel slots for good, so enough stalls
        // permanently shrink the proxy's CONNECT capacity. Shutdown does not
        // rescue them either: the drain gives up when its own timeout elapses
        // and the tracker never aborts what is left.
        //
        // Racing the cancellation token as well as the clock means shutdown
        // reaches a tunnel that is still being set up, not only one that is
        // already running.
        macro_rules! setup_step {
            ($what:literal, $fut:expr) => {
                tokio::select! {
                    r = tokio::time::timeout(HANDSHAKE_TIMEOUT, $fut) => match r {
                        Ok(Ok(v)) => v,
                        Ok(Err(e)) => {
                            if let Some(n) = ABANDONED_TUNNEL.allow(REFUSAL_LOG_INTERVAL) {
                                log::debug!(
                                    concat!(
                                        "CONNECT {} for {target} failed: {e}",
                                        " ({n} similar since the last message)"
                                    ),
                                    $what,
                                    target = target,
                                    e = e,
                                    n = n
                                );
                            }
                            return;
                        }
                        Err(_) => {
                            if let Some(n) = ABANDONED_TUNNEL.allow(REFUSAL_LOG_INTERVAL) {
                                log::debug!(
                                    concat!(
                                        "CONNECT {} for {target} did not finish within",
                                        " {timeout:?}",
                                        " ({n} similar since the last message)"
                                    ),
                                    $what,
                                    target = target,
                                    timeout = HANDSHAKE_TIMEOUT,
                                    n = n
                                );
                            }
                            return;
                        }
                    },
                    // Not throttled: shutdown happens once, so the count is
                    // bounded by the tunnels open at that moment.
                    _ = cancel.cancelled() => {
                        log::debug!("CONNECT {} for {target} abandoned by shutdown", $what);
                        return;
                    }
                }
            };
        }

        let upgraded = setup_step!("upgrade", hyper::upgrade::on(req));
        let mut client = hyper_util::rt::TokioIo::new(upgraded);
        let mut server = setup_step!("connect", tokio::net::TcpStream::connect(target));

        // Straight byte splice: the payload is TLS pitchfork must not and
        // cannot read here. Shutdown drops both sockets rather than waiting
        // for the peers to finish, since a tunnel can legitimately stay open
        // for hours.
        tokio::select! {
            r = tokio::io::copy_bidirectional(&mut client, &mut server) => {
                // Throttled like the setup steps: a client can end tunnels
                // with errors as fast as it can open them.
                if let Err(e) = r
                    && let Some(n) = ABANDONED_TUNNEL.allow(REFUSAL_LOG_INTERVAL)
                {
                    log::debug!(
                        "CONNECT tunnel to {target} ended: {e} ({n} similar since the last message)"
                    );
                }
            }
            _ = cancel.cancelled() => {
                log::debug!("CONNECT tunnel to {target} closed by shutdown");
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(PITCHFORK_HEADER, "1")
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Get the effective host from a request.
///
/// HTTP/2 uses the `:authority` pseudo-header, which hyper exposes via
/// `req.uri().authority()` rather than in the `HeaderMap`.
/// HTTP/1.1 uses the `Host` header.
fn get_request_host(req: &Request) -> Option<String> {
    // HTTP/2: :authority is available via the request URI, not the HeaderMap.
    let authority = req
        .uri()
        .authority()
        .map(|a| a.as_str().to_string())
        .filter(|s| !s.is_empty());

    authority.or_else(|| {
        req.headers()
            .get(HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_string)
    })
}

/// Rejoin a `cookie` header that arrived split across several fields.
///
/// An HTTP/2 client may send each cookie as its own header field (RFC 9113
/// §8.2.3). An HTTP/1.1 backend joins repeated fields with `", "`, which
/// corrupts every cookie value, so they must be joined with `"; "` first.
fn join_cookie_fields(headers: &mut HeaderMap) {
    let fields: Vec<&[u8]> = headers
        .get_all(COOKIE)
        .iter()
        .map(HeaderValue::as_bytes)
        .collect();
    if fields.len() < 2 {
        return;
    }

    let joined = HeaderValue::from_bytes(&fields.join(b"; ".as_slice()))
        .expect("valid header values joined with \"; \" form a valid header value");
    headers.insert(COOKIE, joined);
}

/// Inject `X-Forwarded-*` headers into a proxied request.
///
/// Because the proxy is a **first-hop** dev tool (not a mid-tier forwarder),
/// all four headers are **unconditionally overwritten** with values derived
/// from the actual incoming connection.  Any values supplied by the connecting
/// client are discarded.
///
/// Trusting client-supplied `x-forwarded-for` / `x-forwarded-proto` would
/// allow a local process to spoof a remote IP or trick a backend's
/// HTTPS-detection logic (CSRF checks, secure-cookie flags, redirect rules).
fn inject_forwarded_headers(req: &mut Request, is_tls: bool, host_header: &str) {
    let remote_addr = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let proto = if is_tls { "https" } else { "http" };
    let default_port = if is_tls { "443" } else { "80" };

    // Always set fresh values — we are the edge, never a mid-tier forwarder.
    // Discard any x-forwarded-* headers supplied by the connecting client.
    let forwarded_for = remote_addr.clone();
    let forwarded_proto = proto.to_string();
    let forwarded_host = host_header.to_string();
    let forwarded_port = host_header
        .rsplit_once(':')
        .map(|(_, port)| port.to_string())
        .unwrap_or_else(|| default_port.to_string());

    // Strip any client-supplied x-forwarded-* and RFC 7239 Forwarded headers
    // before inserting ours, so that no trace of the original values reaches
    // the backend.  The RFC 7239 `Forwarded` header is stripped alongside the
    // legacy `x-forwarded-*` set because backends that read it (Django, Rails,
    // Spring) would otherwise see client-injected spoofed IPs or protocols.
    for name in [
        "x-forwarded-for",
        "x-forwarded-proto",
        "x-forwarded-host",
        "x-forwarded-port",
        "forwarded",
    ] {
        if let Ok(header_name) = axum::http::HeaderName::from_bytes(name.as_bytes()) {
            req.headers_mut().remove(&header_name);
        }
    }

    let headers = [
        ("x-forwarded-for", forwarded_for),
        ("x-forwarded-proto", forwarded_proto),
        ("x-forwarded-host", forwarded_host),
        ("x-forwarded-port", forwarded_port),
    ];

    for (name, value) in headers {
        if let Ok(v) = HeaderValue::from_str(&value) {
            let header_name = axum::http::HeaderName::from_static(name);
            req.headers_mut().insert(header_name, v);
        }
    }
}

/// Main proxy request handler.
///
/// Parses the `Host` header, resolves the target daemon, and forwards the request.
/// WebSocket / HTTP upgrade requests are forwarded transparently via hyper's upgrade mechanism.
async fn proxy_handler(State(state): State<ProxyState>, mut req: Request) -> Response {
    // Extract the host (supports both HTTP/2 :authority and HTTP/1.1 Host)
    let Some(raw_host) = get_request_host(&req) else {
        return error_response(StatusCode::BAD_REQUEST, "Missing Host header");
    };

    // A browser configured through the PAC file sends CONNECT for every
    // `https://` URL, so this is the entry point for the whole PAC path under
    // the default HTTPS configuration.
    if req.method() == axum::http::Method::CONNECT {
        return connect_handler(&state, req, &raw_host).await;
    }
    // Strip port from host for routing.
    // IPv6 addresses in Host headers are bracketed per RFC 2732: `[::1]:port`.
    // Splitting naïvely on ':' would break on the colons inside the address.
    let host = if raw_host.starts_with('[') {
        // IPv6: "[::1]:port" or "[::1]"
        raw_host
            .split("]:")
            .next()
            .unwrap_or(&raw_host)
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    } else {
        // IPv4 / hostname: "host:port" or "host"
        raw_host.split(':').next().unwrap_or(&raw_host).to_string()
    };
    // A fully qualified name may end in the root dot (`api.localhost.`). The
    // resolver and the certificate issuer already accept it through
    // `owns_name`, so routing has to as well, or the name resolves and
    // handshakes only to be answered "not found".
    let host = host.trim_end_matches('.').to_string();

    // Loop detection: check hop count.
    //
    // Security: strip (zero out) the hop counter on the very first hop to
    // prevent external clients from forging a high value and triggering a
    // 508 Loop Detected response (denial-of-service).  A request is
    // considered "first hop" when it does not carry the `x-pitchfork-hops`
    // request header that pitchfork injects when forwarding — i.e. it did
    // not come from another pitchfork proxy instance.
    // Note: `x-pitchfork` is a *response* header added by pitchfork and is
    // never present on incoming requests, so it cannot be used here.
    let is_from_pitchfork = req.headers().contains_key(PROXY_HOPS_HEADER);
    let hops: u64 = if is_from_pitchfork {
        req.headers()
            .get(PROXY_HOPS_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    } else {
        // External request: ignore any forged hop counter.
        0
    };
    if hops >= MAX_PROXY_HOPS {
        return error_response(
            StatusCode::LOOP_DETECTED,
            &format!(
                "Loop detected for '{host}': request has passed through the proxy {hops} times.\n\
                 This usually means a backend is proxying back through pitchfork without rewriting \n\
                 the Host header. If you use Vite/webpack proxy, set changeOrigin: true."
            ),
        );
    }

    let local_client = is_local_client(&req);

    // Intercept "pitchfork.<tld>" — route to the built-in web UI
    let target_port = if let Some(subdomain) = strip_tld(&host, &state.tld) {
        if subdomain.eq_ignore_ascii_case("pitchfork") {
            crate::web::port()
        } else {
            None
        }
    } else {
        None
    };

    let target_port = if let Some(port) = target_port {
        port
    } else {
        // A passthrough hostname must never be forwarded as plain HTTP: the
        // daemon expects a TLS handshake on that port, so the request would
        // fail deep inside the daemon with nothing to point at the cause.
        if resolve_tls_mode(&host, &state.tld).await.is_passthrough() {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &passthrough_unroutable_message(&host, state.is_tls),
            );
        }
        match resolve_target(&host, &state.tld).await {
            ResolveResult::Ready(port) => port,
            ResolveResult::Starting { slug } => {
                return starting_html_response(&slug, &raw_host);
            }
            ResolveResult::Page {
                project,
                worktree,
                daemons,
                dir,
            } => {
                // A reserved name answers 200 while an unknown one answers 404,
                // which tells anything on the network which projects exist. Off
                // this machine the two look the same.
                if !local_client {
                    return unknown_host_response(&host, "Not found", &[]);
                }
                // The project and stack pages live in the web UI, which is
                // their canonical location, so this hostname redirects there.
                // The page is found by the checkout the hostname resolved
                // to, because its URL is built from the registered project
                // name and the worktree's own name, neither of which has to
                // match the sanitized, overridable labels in the hostname.
                if let Some(base) = crate::web::url()
                    && let Some(resolved) = dir.clone()
                    && let Some(path) = tokio::task::spawn_blocking(move || {
                        crate::web::routes::api::projects::page_path_for_dir(&resolved)
                    })
                    .await
                    .ok()
                    .flatten()
                {
                    return page_redirect_response(&base, &path);
                }
                return page_placeholder_response(
                    &project,
                    worktree.as_deref(),
                    &daemons,
                    &state.tld,
                    &host_port_suffix(&raw_host),
                    crate::web::url().as_deref(),
                );
            }
            ResolveResult::Unknown { heading, known } => {
                // The heading says which project was recognised, which is one
                // more thing than a remote client needs to learn.
                return unknown_host_response(
                    &host,
                    if local_client { &heading } else { "Not found" },
                    if local_client { &known } else { &[] },
                );
            }
            ResolveResult::NotFound => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!(
                        "No daemon found for host '{host}'.\n\
                         A daemon is reachable once it configures a `port` and its project is \
                         known to pitchfork; run `pitchfork proxy status` to see the hostnames \
                         it serves.\n\
                         Expected format: <daemon>.<project>.{tld}",
                        tld = state.tld
                    ),
                );
            }
            ResolveResult::Error(msg) => {
                if local_client {
                    return error_response(StatusCode::BAD_GATEWAY, &msg);
                }
                // The message names directories on this machine.
                log::warn!("Refused '{host}' for a non-local client: {msg}");
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("'{host}' is not available."),
                );
            }
        }
    };
    // Build the forwarding URI
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let forward_uri = match Uri::builder()
        .scheme("http")
        .authority(format!("localhost:{target_port}"))
        .path_and_query(path_and_query)
        .build()
    {
        Ok(uri) => uri,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to build forward URI: {e}"),
            );
        }
    };

    // Update the request URI and Host header
    *req.uri_mut() = forward_uri;
    req.headers_mut().insert(
        HOST,
        HeaderValue::from_str(&format!("localhost:{target_port}"))
            .unwrap_or_else(|_| HeaderValue::from_static("localhost")),
    );

    // Inject X-Forwarded-* headers
    inject_forwarded_headers(&mut req, state.is_tls, &raw_host);

    // Increment hop counter
    if let Ok(v) = HeaderValue::from_str(&(hops + 1).to_string()) {
        req.headers_mut()
            .insert(axum::http::HeaderName::from_static(PROXY_HOPS_HEADER), v);
    }

    // Explicitly strip HTTP/2 pseudo-headers (":authority", ":method", etc.)
    // before forwarding to an HTTP/1.1 backend. Although hyper typically does
    // not store pseudo-headers in the HeaderMap, some middleware layers or
    // future hyper versions might; stripping them here is a defensive measure.
    let pseudo_headers: Vec<_> = req
        .headers()
        .keys()
        .filter(|k| k.as_str().starts_with(':'))
        .cloned()
        .collect();
    for key in pseudo_headers {
        req.headers_mut().remove(&key);
    }

    join_cookie_fields(req.headers_mut());

    // Downgrade the forwarded request to HTTP/1.1. TLS connections negotiate
    // HTTP/2 inbound via ALPN, but the upstream forward client speaks HTTP/1 to
    // the daemon. Without this, the still-h2-tagged request is rejected by the
    // client with `UserUnsupportedVersion`, surfacing as a 502 to the browser.
    *req.version_mut() = axum::http::Version::HTTP_11;

    // Extract the client-side OnUpgrade handle *before* consuming req
    let client_upgrade = hyper::upgrade::on(&mut req);

    // Forward the request with a per-request timeout so that a backend that
    // accepts the TCP connection but then stalls (deadlock, blocking I/O, etc.)
    // cannot hold the proxy connection open forever and exhaust file descriptors.
    //
    // 120 s is intentionally generous for a local dev proxy — it covers slow
    // test suites, large file uploads, and SSE streams while still bounding
    // the worst-case resource leak.
    let result = match tokio::time::timeout(
        std::time::Duration::from_secs(120),
        state.client.request(req),
    )
    .await
    {
        Ok(r) => r,
        Err(_elapsed) => {
            let msg = format!(
                "Request to daemon on port {target_port} timed out after 120 s.\n\
                 The daemon accepted the connection but did not respond in time."
            );
            log::warn!("{msg}");
            if let Some(ref on_error) = state.on_error {
                on_error(&msg);
            }
            return error_response(StatusCode::GATEWAY_TIMEOUT, &msg);
        }
    };
    match result {
        Ok(mut resp) => {
            // Extract backend upgrade handle *before* consuming resp
            let backend_upgrade = hyper::upgrade::on(&mut resp);
            let (mut parts, body) = resp.into_parts();

            // Add pitchfork identification header
            parts.headers.insert(
                axum::http::HeaderName::from_static(PITCHFORK_HEADER),
                HeaderValue::from_static("1"),
            );

            // Strip the internal hop-counter so it is never leaked to external clients.
            parts.headers.remove(PROXY_HOPS_HEADER);

            // Strip hop-by-hop headers when serving HTTPS (HTTP/2 forbids them).
            // Skip 101 Switching Protocols — that response is always HTTP/1.1 and
            // the client needs the `Upgrade` header to complete the WS handshake
            // (RFC 6455 §4.1 requires `Upgrade: websocket` in the 101 response).
            if state.is_tls && parts.status != StatusCode::SWITCHING_PROTOCOLS {
                for h in HOP_BY_HOP_HEADERS {
                    if let Ok(name) = axum::http::HeaderName::from_bytes(h.as_bytes()) {
                        parts.headers.remove(&name);
                    }
                }
            }

            // If the backend returned 101 Switching Protocols, pipe the upgraded streams.
            if parts.status == StatusCode::SWITCHING_PROTOCOLS {
                // Note: loop detection for WebSocket upgrades is already handled at the
                // top of proxy_handler (hops >= MAX_PROXY_HOPS check) before the request
                // is forwarded.  A 101 response here means the backend accepted the
                // upgrade, so the hop count was already within limits.
                tokio::spawn(async move {
                    if let (Ok(client_upgraded), Ok(backend_upgraded)) =
                        (client_upgrade.await, backend_upgrade.await)
                    {
                        let mut client_io = hyper_util::rt::TokioIo::new(client_upgraded);
                        let mut backend_io = hyper_util::rt::TokioIo::new(backend_upgraded);
                        // No application-level timeout here: tokio::time::timeout would be a
                        // hard wall-clock deadline for the entire tunnel, not an idle timeout.
                        // Long-lived connections (Vite/webpack HMR, SSE-over-WS) would be
                        // silently terminated after the deadline even if data is actively
                        // flowing.  The OS TCP keepalive is sufficient to reap truly dead
                        // connections; a proper idle timeout would require a custom
                        // AsyncRead/AsyncWrite wrapper that resets the timer on each I/O op.
                        let _ =
                            tokio::io::copy_bidirectional(&mut client_io, &mut backend_io).await;
                    }
                });
                return Response::from_parts(parts, Body::empty());
            }

            // Backend refused the upgrade (returned a non-101 response) — forward it as-is.
            // This can happen when the backend rejects a WebSocket handshake with e.g. 400.
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => {
            let msg = format!(
                "Failed to connect to daemon on port {target_port}: {e}\n\
                 The daemon may have stopped or is not yet ready."
            );
            if let Some(ref on_error) = state.on_error {
                on_error(&msg);
            } else {
                log::warn!("{msg}");
            }
            error_response(StatusCode::BAD_GATEWAY, &msg)
        }
    }
}

/// Explain why a request for a `proxy_tls = "passthrough"` hostname arrived on
/// the HTTP path, where it cannot be served.
///
/// Passthrough routes on the hostname in the TLS ClientHello, so there are two
/// ways to end up here: the proxy is not serving TLS at all, or the TLS
/// connection named one host and the request inside it named another.
///
/// A connection with no SNI at all never reaches this point: the certificate
/// resolver has no name to issue for, so the handshake fails outright.
fn passthrough_unroutable_message(host: &str, is_tls: bool) -> String {
    if is_tls {
        format!(
            "'{host}' uses proxy_tls = \"passthrough\", which routes on the host name \
             in the TLS ClientHello.\n\
             This connection's TLS handshake named a different host, so it was \
             terminated here and the request inside it cannot be spliced to the \
             daemon.\n\
             Make the connection itself name '{host}' rather than overriding the Host \
             header of a connection opened to something else."
        )
    } else {
        format!(
            "'{host}' uses proxy_tls = \"passthrough\", but the proxy is serving \
             plain HTTP.\n\
             Passthrough splices a TLS stream to the daemon, so it requires \
             settings.proxy.https = true.\n\
             Enable HTTPS on the proxy, or set proxy_tls = \"terminate\" on the daemon."
        )
    }
}

/// Resolve the target for a given hostname.
///
/// Slug-based routing using the global config's `[slugs]` section:
/// 1. Strip TLD to get subdomain (the slug)
/// 2. Look up slug in global config → find project dir + daemon name
/// 3. Check state file for a running daemon with that name → get its port
/// 4. If `proxy.auto_start` is enabled and the daemon is not running,
///    trigger an automatic start and wait for it to become ready.
///
/// # Returns
/// - `ResolveResult::Ready(port)`       — daemon running (or just auto-started), forward to this port
/// - `ResolveResult::Starting { slug }` — daemon start in progress (show waiting page)
/// - `ResolveResult::NotFound`          — no daemon matched
/// - `ResolveResult::Error(msg)`        — routing refused with a descriptive reason
///
/// # Locking
/// The state file lock is held only for the duration of the snapshot copy,
/// then released immediately to avoid serialising all proxy requests.
async fn resolve_target(host: &str, tld: &str) -> ResolveResult {
    let ctx = match resolve_route_context(host, tld).await {
        Ok(ctx) => ctx,
        Err(result) => return result,
    };

    let daemons = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.clone()
    };

    let daemon_name = &ctx.cached.daemon_name;
    let running_matches: Vec<(&DaemonId, &crate::daemon::Daemon)> = daemons
        .iter()
        .filter(|(id, d)| {
            id.name() == daemon_name
                && d.status.is_running()
                && match &ctx.expected_namespace {
                    Some(ns) => id.namespace() == ns,
                    None => true,
                }
        })
        .collect();

    match running_matches.as_slice() {
        [] => {
            try_auto_start(
                &ctx.cached.slug,
                &ctx.cached,
                ctx.worktree_dir.as_deref(),
                ctx.expected_namespace.as_deref(),
                &ctx.route,
            )
            .await
        }
        // With more than one namespace running a daemon of this name and no
        // namespace to narrow by, the first match is used — as before.
        [(_, d), ..] => match select_daemon_port(&ctx.route, d) {
            Some(port) => ResolveResult::Ready(port),
            None => ResolveResult::NotFound,
        },
    }
}

/// What a hostname resolves to before the daemon's running state is consulted:
/// the slug it matched, the namespace and worktree it names, and how its TLS
/// is handled.
struct RouteContext {
    cached: CachedSlugEntry,
    expected_namespace: Option<String>,
    worktree_dir: Option<std::path::PathBuf>,
    route: ProxyTlsRoute,
}

/// Resolve a hostname to its slug, worktree and TLS route.
///
/// This is the part of routing that does not depend on whether the daemon is
/// running, which is what lets the 443 listener decide between terminating TLS
/// and splicing the raw stream before anything is started.
///
/// Returns `Err(ResolveResult)` when the host does not route at all, carrying
/// the response the caller should produce.
async fn resolve_route_context(host: &str, tld: &str) -> Result<RouteContext, ResolveResult> {
    let Some(subdomain) = strip_tld(host, tld) else {
        return Err(ResolveResult::NotFound);
    };

    let cached = cached_slug_lookup(&subdomain).await.filter(|cached| {
        // A slug too long for the configured TLD is not advertised as a URL, so
        // it does not take precedence over the daemon's automatic hostname
        // here either.
        if crate::proxy::hostname::hostname_fits(&cached.slug) {
            return true;
        }
        crate::proxy::hostname::warn_once(&format!(
            "Slug '{}' plus the configured proxy.tld is over the DNS length limit, \
             so it is not routed.",
            cached.slug
        ));
        false
    });
    let Some(cached) = cached else {
        // No legacy slug matched; fall through to the automatic
        // `<daemon>.<worktree>.<project>` hostnames.
        return Err(resolve_registry_target(&subdomain).await);
    };

    // ─── Worktree prefix extraction ──────────────────────────────────────
    // When a wildcard subdomain like "feature-a.myapp" matched slug "myapp",
    // the prefix "feature-a" may correspond to a git worktree or jj workspace.
    let (expected_namespace, worktree_dir, route) = if !subdomain.eq_ignore_ascii_case(&cached.slug)
    {
        let prefix = strip_dot_suffix_ignore_case(&subdomain, &cached.slug);
        match prefix {
            Some(ref p) => match match_worktree_prefix(&cached, p) {
                PrefixMatch::Worktree(wt) => {
                    let ns = wt.namespace.clone().or_else(|| {
                        log::warn!(
                            "Worktree '{}' has no cached namespace; \
                             falling back to parent slug namespace.",
                            wt.path.display()
                        );
                        cached.namespace.clone()
                    });
                    let route = worktree_route(&cached, &wt.sanitized_branch);
                    (ns, Some(wt.path.clone()), route)
                }
                PrefixMatch::Ambiguous => {
                    return Err(ResolveResult::Error(format!(
                        "'{host}' is ambiguous: more than one branch or workspace of '{slug}' \
                         sanitizes to the prefix '{p}', and host names are case-insensitive.\n\
                         Rename one of them so the prefixes differ by more than case, then \
                         reload.\n\
                         The supervisor log lists the colliding branches.",
                        slug = cached.slug,
                    )));
                }
                PrefixMatch::Unknown => (cached.namespace.clone(), None, cached.tls),
            },
            None => (cached.namespace.clone(), None, cached.tls),
        }
    } else {
        (cached.namespace.clone(), None, cached.tls)
    };

    Ok(RouteContext {
        cached,
        expected_namespace,
        worktree_dir,
        route,
    })
}

/// How the proxy should handle TLS for `host`.
///
/// Answered from the slug cache alone, so it is cheap enough to call on every
/// TLS connection. Anything that does not resolve to a daemon — an unknown
/// host, the built-in web UI, a bare IP — keeps today's behavior and
/// terminates.
pub(crate) async fn resolve_tls_mode(host: &str, tld: &str) -> ProxyTlsMode {
    let entries = get_cached_slugs().await;
    let registry = get_cached_host_registry().await;
    resolve_tls_mode_in(host, tld, &entries, &registry)
}

/// [`resolve_tls_mode`] against a given slug table, without awaiting.
///
/// Used by the certificate resolver, which runs in a synchronous trait method
/// and reads [`slug_snapshot`].
fn resolve_tls_mode_in(
    host: &str,
    tld: &str,
    entries: &std::collections::HashMap<String, CachedSlugEntry>,
    registry: &crate::proxy::hostname::HostRegistry,
) -> ProxyTlsMode {
    let Some(subdomain) = strip_tld(host, tld) else {
        return ProxyTlsMode::Terminate;
    };
    let wildcard = settings().proxy.wildcard;

    // Legacy slugs resolve first, exactly as routing resolves them.
    if let Some(cached) = wildcard_slug_lookup(&subdomain, entries, wildcard)
        && crate::proxy::hostname::hostname_fits(&cached.slug)
    {
        // A wildcard match may name a worktree, which carries its own setting.
        if !subdomain.eq_ignore_ascii_case(&cached.slug)
            && let Some(prefix) = strip_dot_suffix_ignore_case(&subdomain, &cached.slug)
            && let PrefixMatch::Worktree(wt) = match_worktree_prefix(cached, &prefix)
        {
            return worktree_route(cached, &wt.sanitized_branch).mode;
        }
        return cached.tls.mode;
    }

    // Otherwise the automatic `<daemon>.<worktree>.<project>` hostnames, whose
    // mode lives in the config of the checkout the name points at.
    match registry.resolve(&subdomain, wildcard) {
        crate::proxy::hostname::HostTarget::Daemon { proxy_tls, .. } => {
            proxy_tls.unwrap_or_default()
        }
        // A project or stack page, an unknown name: the proxy answers those
        // itself over its own certificate.
        _ => ProxyTlsMode::Terminate,
    }
}

/// Pick which of a running daemon's ports a hostname forwards to.
///
/// Without `proxy_tls_port`, a terminating hostname uses the port the process
/// was detected listening on, falling back to the first resolved port — the
/// historical behavior, and the right answer for a single-port daemon. A
/// passthrough hostname reverses that order, because the first detected
/// listener of a multi-port daemon need not be the one speaking TLS.
///
/// With `proxy_tls_port` set, the configured port is matched by *position* in
/// the daemon's `port` list, so the mapping survives auto-bump: a daemon
/// configured for `[8443, 9443]` whose ports bumped to `[8444, 9444]` still
/// routes `proxy_tls_port = 9443` to 9444. Config validation rejects a
/// `proxy_tls_port` the daemon does not declare, so the only way the position
/// lookup can miss is a state record that predates the current config; the
/// port is then used as written only while the daemon has no resolved ports to
/// contradict it.
fn select_daemon_port(route: &ProxyTlsRoute, daemon: &crate::daemon::Daemon) -> Option<u16> {
    let Some(want) = route.port else {
        // A passthrough hostname has to reach the daemon's *TLS* listener.
        // `active_port` is whichever port was detected first, which on a
        // multi-port daemon can be a secondary plain-HTTP or status listener
        // that came up earlier, so the declared first port wins there — as
        // documented. A terminating hostname keeps preferring the detected
        // port, which is what routes daemons that declare no ports at all.
        // Port 0 is what a daemon carries when it asked the operating system
        // to choose and nothing has been detected yet; it is not connectable.
        let detected = daemon.active_port.filter(|&p| p != 0);
        let first_declared = daemon.resolved_port.iter().copied().find(|&p| p != 0);
        return if route.mode.is_passthrough() {
            first_declared.or(detected)
        } else {
            detected.or(first_declared)
        };
    };

    let configured = daemon
        .port
        .as_ref()
        .map(|p| p.expect.as_slice())
        .unwrap_or(&[]);
    if let Some(idx) = configured.iter().position(|&p| p == want)
        && let Some(&resolved) = daemon.resolved_port.get(idx)
    {
        // A recorded 0 is a port the daemon asked the operating system to
        // choose and nothing has detected yet, so it is no more connectable
        // here than on the path that takes the daemon's first port. That is a
        // daemon still starting, not a mismatch, so it is not warned about:
        // auto-start polls through this state several times a second.
        return (resolved != 0).then_some(resolved);
    }
    if daemon.resolved_port.contains(&want) {
        return Some(want);
    }
    if daemon.resolved_port.is_empty() {
        // Nothing recorded to place the port against — the config is the only
        // information there is, so use it.
        return Some(want);
    }
    // Every request to the hostname lands here until the daemon restarts, so
    // the warning is logged once per daemon and port set, not per request.
    crate::proxy::hostname::warn_once(&format!(
        "Daemon {} has proxy_tls_port {want}, which is not among its resolved ports {:?}; \
         refusing to route rather than forwarding to a port it never bound. \
         Restart the daemon if its ports changed.",
        daemon.id, daemon.resolved_port,
    ));
    None
}

/// RAII guard that removes a `DaemonId` from `AUTO_START_IN_PROGRESS` on drop.
///
/// This ensures the in-progress flag is cleared even if the auto-start future
/// panics (e.g. an unexpected `unwrap` inside a dependency).  Without this,
/// the daemon ID would stay in the set permanently and every subsequent proxy
/// request would return "Starting …" forever.
struct AutoStartGuard {
    daemon_id: DaemonId,
}

impl Drop for AutoStartGuard {
    fn drop(&mut self) {
        let daemon_id = self.daemon_id.clone();
        // Spawn a cleanup task because `Drop` is synchronous and the mutex is
        // async.  If the runtime is shutting down this may not execute, but in
        // that case the entire set is being dropped anyway.
        tokio::spawn(async move {
            AUTO_START_IN_PROGRESS.lock().await.remove(&daemon_id);
        });
    }
}

/// Attempt to auto-start a daemon for the given slug.
///
/// If `proxy.auto_start` is disabled, returns `NotFound`.
/// Uses a dedup set to prevent concurrent starts for the same daemon.
/// Calls `SUPERVISOR.run()` with `wait_ready = true` so the daemon goes
/// through the same readiness lifecycle as `pf start`, then polls for the
/// active port.
///
/// The entire operation — including `SUPERVISOR.run()` and the port-polling
/// loop — is bounded by `proxy_auto_start_timeout`.
async fn try_auto_start(
    slug: &str,
    cached: &CachedSlugEntry,
    worktree_dir: Option<&std::path::Path>,
    expected_namespace: Option<&str>,
    route: &ProxyTlsRoute,
) -> ResolveResult {
    let s = settings();
    if !s.proxy.auto_start {
        return ResolveResult::NotFound;
    }

    let ns = expected_namespace
        .map(|s| s.to_string())
        .or_else(|| cached.namespace.clone())
        .unwrap_or_else(|| "global".to_string());
    let daemon_id = match DaemonId::try_new(&ns, &cached.daemon_name) {
        Ok(id) => id,
        Err(_) => return ResolveResult::NotFound,
    };

    {
        let mut in_progress = AUTO_START_IN_PROGRESS.lock().await;
        if !in_progress.insert(daemon_id.clone()) {
            return ResolveResult::Starting {
                slug: slug.to_string(),
            };
        }
    }

    let _guard = AutoStartGuard {
        daemon_id: daemon_id.clone(),
    };

    let timeout = s.proxy_auto_start_timeout();

    match tokio::time::timeout(
        timeout,
        try_auto_start_inner(slug, cached, &daemon_id, worktree_dir, route),
    )
    .await
    {
        Ok(result) => result,
        Err(_elapsed) => {
            log::warn!("Auto-start: total timeout ({timeout:?}) exceeded for daemon {daemon_id}");
            ResolveResult::Error(format!(
                "Auto-start for '{daemon_id}' timed out after {timeout:?}.\n\
                 The daemon did not become ready and bind a port within the configured \
                 proxy_auto_start_timeout.\n\
                 Increase the timeout or check the daemon's logs for slow startup."
            ))
        }
    }
}

/// Inner implementation of [`try_auto_start`] extracted so that the caller can
/// wrap it with `tokio::time::timeout` and unconditionally clean up
/// `AUTO_START_IN_PROGRESS` regardless of the outcome.
async fn try_auto_start_inner(
    slug: &str,
    cached: &CachedSlugEntry,
    daemon_id: &DaemonId,
    worktree_dir: Option<&std::path::Path>,
    route: &ProxyTlsRoute,
) -> ResolveResult {
    let config_dir = worktree_dir.unwrap_or(&cached.dir);

    let pt = match crate::pitchfork_toml::PitchforkToml::all_merged_from(config_dir) {
        Ok(pt) => pt,
        Err(e) => {
            log::warn!(
                "Auto-start: failed to load config from {}: {e}",
                config_dir.display()
            );
            return ResolveResult::NotFound;
        }
    };

    let mut daemon_config = match pt.daemons.get(daemon_id) {
        Some(cfg) => cfg.clone(),
        None => {
            log::debug!(
                "Auto-start: daemon {daemon_id} not found in config at {}",
                config_dir.display()
            );
            return ResolveResult::NotFound;
        }
    };

    // Render Tera templates and merge top-level env (per-daemon wins). Building
    // the template context reads configuration and derives hostnames, so it
    // runs on a blocking worker rather than on the thread serving the request.
    let rendered = {
        let id = daemon_id.clone();
        let mut config = daemon_config.clone();
        tokio::task::spawn_blocking(move || {
            crate::ipc::batch::render_daemon_config(&id, &mut config, &pt).map(|()| config)
        })
        .await
    };
    daemon_config = match rendered {
        Ok(Ok(config)) => config,
        Ok(Err(e)) => {
            log::warn!("Auto-start: failed to render templates for {daemon_id}: {e}");
            return ResolveResult::Error(format!("Failed to render templates: {e}"));
        }
        Err(e) => {
            log::warn!("Auto-start: template rendering task failed for {daemon_id}: {e}");
            return ResolveResult::Error(format!("Failed to render templates: {e}"));
        }
    };

    let opts = crate::ipc::batch::StartOptions {
        quiet: true,
        ..crate::ipc::batch::StartOptions::default()
    };
    let mut run_opts =
        match crate::ipc::batch::build_run_options(daemon_id, &daemon_config, Some(&opts)).await {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Auto-start: failed to build run options for {daemon_id}: {e}");
                return ResolveResult::Error(format!("Failed to build run options: {e}"));
            }
        };

    // Only set the working directory when the daemon config didn't specify one.
    // If the config has an explicit `dir`, respect it even in a worktree context.
    if run_opts.dir.0.as_os_str().is_empty() {
        run_opts.dir = crate::config_types::Dir(config_dir.to_path_buf());
    }

    log::info!("Auto-start: starting daemon {daemon_id} for slug '{slug}'");

    let run_result = SUPERVISOR.run(run_opts).await;

    if let Err(e) = run_result {
        log::warn!("Auto-start: failed to start daemon {daemon_id}: {e}");
        return ResolveResult::Error(format!("Failed to start daemon: {e}"));
    }

    let poll_interval = std::time::Duration::from_millis(250);

    loop {
        let daemons = {
            let sf = SUPERVISOR.state_file.lock().await;
            sf.daemons.clone()
        };

        if let Some(d) = daemons.get(daemon_id) {
            if d.status.is_running() {
                // Selected through the same route as an already-running daemon,
                // so a hostname with `proxy_tls_port` lands on its configured
                // port on the request that started the daemon, not only on
                // later ones.
                if let Some(port) = select_daemon_port(route, d) {
                    log::info!("Auto-start: daemon {daemon_id} is ready on port {port}");
                    return ResolveResult::Ready(port);
                }
            } else {
                log::warn!(
                    "Auto-start: daemon {daemon_id} is no longer running (status: {})",
                    d.status
                );
                return ResolveResult::Error(format!(
                    "Daemon '{daemon_id}' started but exited unexpectedly.\n\
                     Check its logs for errors."
                ));
            }
        } else {
            log::warn!("Auto-start: daemon {daemon_id} not found in state file after start");
            return ResolveResult::Error(format!(
                "Daemon '{daemon_id}' started but disappeared from the state file.\n\
                 Check its logs for errors."
            ));
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Resolve a hostname against the automatic hostname registry.
///
/// Runs only after the legacy `[slugs]` registry found no match, so a slug
/// keeps precedence over an automatic hostname that spells the same thing.
async fn resolve_registry_target(subdomain: &str) -> ResolveResult {
    let registry = get_cached_host_registry().await;
    if !crate::proxy::hostname::hostname_fits(subdomain) {
        // Nothing advertises a name this long, so nothing answers to one.
        return ResolveResult::Unknown {
            heading: "Host name too long".to_string(),
            known: registry.project_labels(),
        };
    }
    match registry.resolve(subdomain, settings().proxy.wildcard) {
        crate::proxy::hostname::HostTarget::Daemon {
            ref dir,
            ref namespace,
            ref daemon,
            proxy_tls,
            proxy_tls_port,
            ..
        } => {
            // The mode and port were captured with the hostname, from one read
            // of the checkout's config, so a config that has since become
            // unreadable cannot turn a passthrough daemon into a terminated
            // one.
            let route = ProxyTlsRoute {
                mode: proxy_tls.unwrap_or_default(),
                port: proxy_tls_port,
            };
            // When several checkouts share this daemon's namespace — in this
            // project or in another one, since namespaces come from directory
            // names — the ID no longer says which checkout is running, so the
            // request has to be matched to the directory it named.
            let per_checkout = registry.shares_daemon_id(namespace, daemon);
            resolve_registry_daemon(subdomain, dir, namespace, daemon, per_checkout, &route).await
        }
        crate::proxy::hostname::HostTarget::ProjectPage { project } => {
            let entry = registry.projects.get(&project);
            ResolveResult::Page {
                daemons: entry.map(|p| p.primary.labels()).unwrap_or_default(),
                dir: entry.map(|p| p.primary.dir.clone()),
                project,
                worktree: None,
            }
        }
        crate::proxy::hostname::HostTarget::WorktreePage { project, worktree } => {
            let checkout = registry
                .projects
                .get(&project)
                .and_then(|p| p.worktrees.get(&worktree));
            ResolveResult::Page {
                daemons: checkout.map(|c| c.labels()).unwrap_or_default(),
                dir: checkout.map(|c| c.dir.clone()),
                project,
                worktree: Some(worktree),
            }
        }
        crate::proxy::hostname::HostTarget::UnknownProject { known } => ResolveResult::Unknown {
            heading: "Unknown project".to_string(),
            known,
        },
        crate::proxy::hostname::HostTarget::UnknownDaemon {
            project,
            worktree,
            known,
        } => ResolveResult::Unknown {
            heading: match worktree {
                Some(wt) => format!("Unknown daemon in '{wt}' of project '{project}'"),
                None => format!("Unknown daemon in project '{project}'"),
            },
            known,
        },
    }
}

/// Find the running daemon behind an automatic hostname, auto-starting it when
/// it is not running.
///
/// Several checkouts of one project can share a namespace when the project
/// declares one explicitly, so a daemon running in the matching directory is
/// preferred over one that merely shares the name.
async fn resolve_registry_daemon(
    host: &str,
    dir: &std::path::Path,
    namespace: &str,
    daemon: &str,
    per_checkout: bool,
    route: &ProxyTlsRoute,
) -> ResolveResult {
    let daemons = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.clone()
    };

    let mut matches: Vec<crate::daemon::Daemon> = daemons
        .iter()
        .filter(|(id, d)| {
            id.name() == daemon && id.namespace() == namespace && d.status.is_running()
        })
        .map(|(_, d)| d.clone())
        .collect();
    // Attributing a daemon to a checkout walks the filesystem, so it happens off
    // the request's worker thread.
    matches = sort_by_checkout(matches, dir).await;

    if let Some(d) = matches.first() {
        // A running daemon from another checkout would serve that checkout's
        // content under this one's hostname, so say what is wrong instead.
        if per_checkout && !runs_in_checkout(d.clone(), dir).await {
            return ResolveResult::Error(format!(
                "'{host}' belongs to the checkout at {}, but daemon '{namespace}/{daemon}' is \
                 running from {}.\n\
                 These checkouts share the namespace '{namespace}', so pitchfork cannot run \
                 both copies at once.\n\
                 Give each checkout its own top-level `namespace`, or stop the other one first.",
                dir.display(),
                d.dir
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "an unknown directory".to_string()),
            ));
        }
        return match select_daemon_port(route, d) {
            Some(port) => ResolveResult::Ready(port),
            None => ResolveResult::NotFound,
        };
    }

    let cached = CachedSlugEntry {
        slug: host.to_string(),
        namespace: Some(namespace.to_string()),
        daemon_name: daemon.to_string(),
        dir: dir.to_path_buf(),
        worktrees: vec![],
        rejected_worktree_prefixes: std::collections::HashSet::new(),
        tls: *route,
        worktree_tls: std::collections::HashMap::new(),
    };
    let result = try_auto_start(host, &cached, None, Some(namespace), route).await;

    // The start can land on a record another checkout already owns, because the
    // supervisor refuses to run a second daemon under the same ID. Serving that
    // port would hand this hostname the other checkout's content.
    if per_checkout && let ResolveResult::Ready(_) = result {
        let started = {
            let state_file = SUPERVISOR.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .find(|(id, _)| id.name() == daemon && id.namespace() == namespace)
                .map(|(_, d)| d.clone())
        };
        if let Some(d) = started
            && !runs_in_checkout(d.clone(), dir).await
        {
            return ResolveResult::Error(format!(
                "'{host}' belongs to the checkout at {}, but daemon '{namespace}/{daemon}' is \
                 running from {}.\n\
                 These checkouts share the namespace '{namespace}', so pitchfork cannot run \
                 both copies at once.\n\
                 Give each checkout its own top-level `namespace`, or stop the other one first.",
                dir.display(),
                d.dir
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "an unknown directory".to_string()),
            ));
        }
    }

    result
}

/// Whether the request came from this machine.
///
/// Names of other people's projects, daemon labels and absolute paths are
/// details of the developer's machine. They help whoever is sitting at it and
/// tell a device on the LAN things it has no business knowing, so pages spell
/// them out for loopback clients only.
fn is_local_client(req: &Request) -> bool {
    // A request whose peer is unknown is treated as remote: withholding detail
    // from a local client is a small loss, and the reverse is a leak.
    req.extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .is_some_and(|ci| ci.0.ip().is_loopback())
}

/// Whether a daemon is running from this checkout.
///
/// The daemon's directory is resolved to the checkout that contains it rather
/// than compared as a path prefix, so a worktree nested inside its primary
/// checkout is attributed to the worktree, and a symlinked or non-canonical
/// directory still matches. A daemon whose explicit `dir` lies outside every
/// checkout belongs to none of them, which keeps it reachable as long as its
/// hostname is unambiguous.
fn daemon_runs_in(daemon: &crate::daemon::Daemon, checkout: &std::path::Path) -> bool {
    daemon
        .dir
        .as_deref()
        .is_some_and(|d| crate::proxy::hostname::checkout_root_of(d) == checkout)
}

/// [`daemon_runs_in`] off the async worker, since it walks the filesystem.
async fn runs_in_checkout(daemon: crate::daemon::Daemon, checkout: &std::path::Path) -> bool {
    let checkout = checkout.to_path_buf();
    tokio::task::spawn_blocking(move || daemon_runs_in(&daemon, &checkout))
        .await
        .unwrap_or(false)
}

/// Order the candidates so that daemons running in this checkout come first.
///
/// Only the attribution runs off-thread, and the candidates stay here, so a
/// failure in that task costs the ordering rather than the candidates
/// themselves.
async fn sort_by_checkout(
    daemons: Vec<crate::daemon::Daemon>,
    checkout: &std::path::Path,
) -> Vec<crate::daemon::Daemon> {
    if daemons.len() < 2 {
        return daemons;
    }
    let dirs: Vec<Option<std::path::PathBuf>> = daemons.iter().map(|d| d.dir.clone()).collect();
    let checkout = checkout.to_path_buf();
    let here = tokio::task::spawn_blocking(move || {
        dirs.iter()
            .map(|dir| {
                dir.as_deref()
                    .is_some_and(|d| crate::proxy::hostname::checkout_root_of(d) == checkout)
            })
            .collect::<Vec<bool>>()
    })
    .await;

    match here {
        Ok(here) => {
            let mut ordered: Vec<(bool, crate::daemon::Daemon)> =
                here.into_iter().zip(daemons).collect();
            ordered.sort_by_key(|(here, _)| !here);
            ordered.into_iter().map(|(_, d)| d).collect()
        }
        Err(e) => {
            log::warn!("Checkout attribution task failed: {e}");
            daemons
        }
    }
}

/// Escape the five characters that change the meaning of HTML text.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Wrap body markup in the shared pitchfork page chrome.
fn html_page(status: StatusCode, title: &str, body: String) -> Response {
    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title} — pitchfork</title>
    <style>
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background: #0f1117;
            color: #e1e4e8;
            display: flex;
            align-items: center;
            justify-content: center;
            min-height: 100vh;
        }}
        .container {{ max-width: 640px; padding: 2rem; }}
        h1 {{ font-size: 1.5rem; font-weight: 600; margin-bottom: 0.75rem; }}
        p {{ color: #8b949e; font-size: 0.9rem; margin-bottom: 0.75rem; }}
        ul {{ list-style: none; margin: 0.5rem 0 1rem; }}
        li {{ margin: 0.25rem 0; }}
        code, a {{
            color: #58a6ff;
            font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
            text-decoration: none;
        }}
    </style>
</head>
<body>
    <div class="container">{body}</div>
</body>
</html>"##
    );
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap_or_else(|_| (status, title.to_string()).into_response())
}

/// The `:port` part of a Host header, or an empty string when it carries none.
///
/// Links on pitchfork's own pages keep the port the request arrived on, so they
/// still work when the proxy listens somewhere other than 80 or 443.
fn host_port_suffix(raw_host: &str) -> String {
    let port = if raw_host.starts_with('[') {
        raw_host.split_once("]:").map(|(_, port)| port)
    } else {
        raw_host.rsplit_once(':').map(|(_, port)| port)
    };
    port.filter(|p| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty())
        .map(|p| format!(":{p}"))
        .unwrap_or_default()
}

/// Redirect a reserved project or stack hostname to its page in the web UI.
///
/// The page lives at `/projects/<project>[/<worktree>]`, which is its canonical
/// location. Both labels come from hostname labels, so they are already limited
/// to characters that need no escaping in a path.
fn page_redirect_response(base: &str, path: &str) -> Response {
    let target = format!("{base}{path}");
    Response::builder()
        .status(StatusCode::FOUND)
        .header(axum::http::header::LOCATION, &target)
        .header(axum::http::header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(format!(
            "This page is at {target}\n"
        )))
        .unwrap_or_else(|_| {
            html_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "pitchfork",
                String::new(),
            )
        })
}

/// Serve the placeholder for a reserved project or stack hostname.
///
/// `<project>.<tld>` and `<worktree>.<project>.<tld>` belong to the project and
/// stack pages, which the web UI serves. This stands in when the web UI is not
/// running, so the hostname never resolves to whichever daemon shares its
/// name.
fn page_placeholder_response(
    project: &str,
    worktree: Option<&str>,
    daemons: &[String],
    tld: &str,
    port_suffix: &str,
    web_url: Option<&str>,
) -> Response {
    let heading = match worktree {
        Some(wt) => format!("{} · {}", escape_html(project), escape_html(wt)),
        None => escape_html(project),
    };
    let suffix = match worktree {
        Some(wt) => format!(
            "{}.{}.{}",
            escape_html(wt),
            escape_html(project),
            escape_html(tld)
        ),
        None => format!("{}.{}", escape_html(project), escape_html(tld)),
    };
    let list = if daemons.is_empty() {
        "<p>No daemon in this checkout has a port configured.</p>".to_string()
    } else {
        let items: String = daemons
            .iter()
            .map(|d| {
                let d = escape_html(d);
                format!("<li><a href=\"//{d}.{suffix}{port_suffix}\">{d}.{suffix}</a></li>")
            })
            .collect();
        format!("<p>Daemons here:</p><ul>{items}</ul>")
    };
    let page = if worktree.is_some() {
        "stack"
    } else {
        "project"
    };
    // The web UI has a page for a checkout only when a registered namespace
    // covers its directory, while a hostname is reserved for any checkout the
    // proxy knows, including ones known only from a slug or the state file. The
    // two cases need different advice.
    let explanation = match web_url {
        Some(url) => format!(
            "<p>This address is reserved for the {page} page, which the web UI serves.              No registered project covers this checkout, so it has no page yet: add its              directory under <code>[namespaces]</code> in your user config, or run              <code>pitchfork proxy add</code> from it.              <a href=\"{url}/projects\">Open the project list</a>.</p>",
            url = escape_html(url),
        ),
        None => format!(
            "<p>This address is reserved for the {page} page, which the web UI serves.              Enable it with <code>[settings.web] auto_start = true</code> to open this              address.</p>"
        ),
    };
    let body = format!("<h1>{heading}</h1>{explanation}{list}");
    html_page(StatusCode::OK, "pitchfork", body)
}

/// Serve the 404 page for a hostname whose project or daemon does not exist.
fn unknown_host_response(host: &str, heading: &str, known: &[String]) -> Response {
    let list = if known.is_empty() {
        "<p>Nothing is registered under this name yet.</p>".to_string()
    } else {
        let items: String = known
            .iter()
            .map(|k| format!("<li><code>{}</code></li>", escape_html(k)))
            .collect();
        format!("<p>Known names:</p><ul>{items}</ul>")
    };
    let body = format!(
        "<h1>{heading}</h1><p>No route for <code>{host}</code>.</p>{list}",
        heading = escape_html(heading),
        host = escape_html(host),
    );
    html_page(StatusCode::NOT_FOUND, "Not found", body)
}

/// Strip the TLD suffix from a hostname, returning the subdomain part.
///
/// Host names are case-insensitive (RFC 4343) and a browser passes on whatever
/// the user typed, so `API.MyProject.LOCALHOST` has to lose its TLD like any
/// other spelling. A trailing root dot names the same host and is accepted too:
/// this is the one place every routing path passes a host name through, whether
/// it came from a `Host` header, an HTTP/2 `:authority`, a peeked ClientHello,
/// or rustls handing over the SNI name it parsed, and only some of those are
/// normalized by the time they arrive.
///
/// Examples:
/// - `api.myproject.localhost` with tld `localhost` → `api.myproject`
/// - `API.LocalHost.` with tld `localhost` → `API`
/// - `localhost` with tld `localhost` → `None` (no subdomain)
fn strip_tld(host: &str, tld: &str) -> Option<String> {
    strip_dot_suffix_ignore_case(host.trim_end_matches('.'), tld)
}

/// Build a human-friendly error message for port binding failures.
fn bind_error_message(port: u16, err: &std::io::Error) -> String {
    if port < 1024 {
        format!(
            "Failed to bind proxy server to port {port}: {err}\n\
             Hint: ports below 1024 require elevated privileges. Run \
             `pitchfork proxy setup`, which grants the bind capability on Linux, \
             or set an unprivileged proxy.port and let setup redirect {port} to it."
        )
    } else {
        format!(
            "Failed to bind proxy server to port {port}: {err}\n\
             Hint: another process may already be using this port."
        )
    }
}

/// Build an HTML "Starting…" response that auto-refreshes every 2 seconds.
///
/// Displayed when a proxy request triggers an auto-start for a stopped daemon.
/// Once the daemon is ready, the next refresh will proxy normally to the backend.
fn starting_html_response(slug: &str, raw_host: &str) -> Response {
    let escaped_slug = slug
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;");
    let escaped_host = raw_host
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;");

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta http-equiv="refresh" content="2">
    <title>Starting {escaped_slug}… — pitchfork</title>
    <style>
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background: #0f1117;
            color: #e1e4e8;
            display: flex;
            align-items: center;
            justify-content: center;
            min-height: 100vh;
        }}
        .container {{
            text-align: center;
            max-width: 480px;
            padding: 2rem;
        }}
        .spinner {{
            width: 48px;
            height: 48px;
            border: 4px solid rgba(255, 255, 255, 0.1);
            border-top-color: #58a6ff;
            border-radius: 50%;
            animation: spin 0.8s linear infinite;
            margin: 0 auto 1.5rem;
        }}
        @keyframes spin {{
            to {{ transform: rotate(360deg); }}
        }}
        h1 {{
            font-size: 1.5rem;
            font-weight: 600;
            margin-bottom: 0.5rem;
        }}
        .slug {{
            color: #58a6ff;
            font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
        }}
        .host {{
            color: #8b949e;
            font-size: 0.875rem;
            margin-top: 0.25rem;
        }}
        .hint {{
            color: #8b949e;
            font-size: 0.8rem;
            margin-top: 1.5rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="spinner"></div>
        <h1>Starting <span class="slug">{escaped_slug}</span>…</h1>
        <p class="host">{escaped_host}</p>
        <p class="hint">This page will refresh automatically when the daemon is ready.</p>
    </div>
</body>
</html>"##
    );

    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header("content-type", "text/html; charset=utf-8")
        .header("retry-after", "2")
        .body(Body::from(html))
        .unwrap_or_else(|_| (StatusCode::SERVICE_UNAVAILABLE, "Starting…").into_response())
}

/// Handler that redirects plain-HTTP requests to HTTPS.
///
/// Used when the proxy is configured for HTTPS but receives a plain-HTTP
/// request on the same port (after the first-byte peek determines it is
/// not a TLS ClientHello).  Returns a 302 redirect to the HTTPS equivalent.
///
/// WebSocket upgrade attempts over plain HTTP are rejected with 400
/// because WS-over-plain-HTTP to a TLS port is inherently broken.
async fn redirect_to_https_handler(req: Request) -> Response {
    // Reject WebSocket upgrades over plain HTTP
    if req.headers().contains_key("upgrade") {
        log::warn!("Dropping plain-HTTP WebSocket upgrade attempt — use wss:// instead of ws://");
        return (
            StatusCode::BAD_REQUEST,
            "WebSocket over plain HTTP is not supported on the HTTPS port. Use wss:// instead.",
        )
            .into_response();
    }

    let raw_host = get_request_host(&req);
    let Some(raw_host) = raw_host else {
        return (StatusCode::BAD_REQUEST, "Missing Host header").into_response();
    };

    // Strip any incoming port from Host and use the configured HTTPS port.
    let hostname = if raw_host.starts_with('[') {
        // IPv6: "[::1]:port" or "[::1]"
        raw_host
            .split_once("]:")
            .map(|(host, _)| host)
            .unwrap_or(&raw_host)
            .trim_start_matches('[')
            .trim_end_matches(']')
    } else {
        // IPv4/hostname: "host:port" or "host"
        let mut parts = raw_host.rsplitn(2, ':');
        let last = parts.next().unwrap_or(&raw_host);
        parts.next().unwrap_or(last)
    };

    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let https_port = match u16::try_from(settings().proxy.port).ok().filter(|&p| p > 0) {
        Some(443) | None => String::new(),
        Some(port) => format!(":{port}"),
    };

    let host_for_url = if raw_host.starts_with('[') {
        format!("[{hostname}]")
    } else {
        hostname.to_string()
    };

    let location = format!("https://{host_for_url}{https_port}{path}");
    (
        StatusCode::FOUND,
        [
            (axum::http::header::LOCATION, location),
            // Identifies the redirect as pitchfork's, so a probe on the port
            // can tell this listener apart from an unrelated service.
            (
                axum::http::HeaderName::from_static(PITCHFORK_HEADER),
                "1".to_string(),
            ),
        ],
    )
        .into_response()
}

/// Build a plain-text error response.
fn error_response(status: StatusCode, message: &str) -> Response {
    // Carries the identification header like any other proxy response: a
    // client — `pitchfork proxy doctor` included — should be able to tell that
    // pitchfork answered even when the answer is an error.
    (
        status,
        [(
            axum::http::HeaderName::from_static(PITCHFORK_HEADER),
            HeaderValue::from_static("1"),
        )],
        message.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axum answers a path match with no method match itself, so a route
    /// registered for `GET` alone takes that path away from `fallback` for
    /// every other method. `/proxy.pac` has to stay reachable on a proxied
    /// hostname under any method, so it is registered with `any` and the
    /// handler decides. This pins the router behaviour that choice rests on.
    #[tokio::test]
    async fn a_get_only_route_never_reaches_the_fallback() {
        async fn routed() -> &'static str {
            "routed"
        }
        async fn fell_through() -> &'static str {
            "fell-through"
        }

        // A hand-written request over a plain socket: no HTTP client, so no
        // TLS provider to install for a test that never uses TLS.
        async fn post_to(app: Router) -> String {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
            sock.write_all(
                b"POST /proxy.pac HTTP/1.1\r\nHost: example\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
            let mut raw = Vec::new();
            sock.read_to_end(&mut raw).await.unwrap();
            server.abort();

            let text = String::from_utf8_lossy(&raw).into_owned();
            let status = text.split_whitespace().nth(1).unwrap_or("").to_string();
            if status == "405" {
                return status;
            }
            text.rsplit("\r\n").next().unwrap_or("").to_string()
        }

        let get_only = Router::new()
            .route("/proxy.pac", axum::routing::get(routed))
            .fallback(fell_through);
        assert_eq!(
            post_to(get_only).await,
            "405",
            "axum reached the fallback on a method mismatch, so `any` is unnecessary"
        );

        let any_method = Router::new()
            .route("/proxy.pac", axum::routing::any(routed))
            .fallback(fell_through);
        assert_eq!(
            post_to(any_method).await,
            "routed",
            "`any` did not deliver the POST to the handler"
        );
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn a_ca_pair_is_checked_not_just_found() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = (dir.path().join("ca.pem"), dir.path().join("ca-key.pem"));
        generate_ca(&cert, &key).unwrap();
        assert_eq!(ca_pair_problem(&cert, &key), None);

        // A key from a different CA.
        let (other_cert, other_key) = (dir.path().join("b.pem"), dir.path().join("b-key.pem"));
        generate_ca(&other_cert, &other_key).unwrap();
        assert!(ca_pair_problem(&cert, &other_key).is_some());

        // A truncated certificate.
        std::fs::write(&cert, "-----BEGIN CERTIFICATE-----\nAAAA\n").unwrap();
        assert!(ca_pair_problem(&cert, &key).is_some());
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn cert_cache_file_names_do_not_collide() {
        // `.` used to become `_`, so these two shared a file.
        assert_ne!(
            cert_cache_file_stem("a_b.localhost"),
            cert_cache_file_stem("a.b.localhost")
        );
        assert_eq!(cert_cache_file_stem("api.localhost"), "api.localhost");
        assert_eq!(cert_cache_file_stem("a_b.localhost"), "a%5Fb.localhost");
        assert_eq!(
            cert_cache_file_stem("*.proj.localhost"),
            "%2A.proj.localhost"
        );
        // No path separators survive.
        assert!(!cert_cache_file_stem("../x").contains('/'));
    }

    #[test]
    fn connect_port_reads_the_authority() {
        assert_eq!(connect_port("api.localhost:443"), Some(443));
        assert_eq!(connect_port("api.localhost:80"), Some(80));
        assert_eq!(connect_port("[::1]:443"), Some(443));
        assert_eq!(connect_port("api.localhost"), None);
        assert_eq!(connect_port("::1"), None);
    }

    #[test]
    fn a_half_configured_certificate_pair_is_refused() {
        // Either half alone would be filled in from the generated CA: the
        // user's key signing with pitchfork's certificate, or a new CA key
        // written over the user's file.
        assert!(tls_pair_problem("", "/k.pem").is_some());
        assert!(tls_pair_problem("/c.pem", "").is_some());
        assert!(tls_pair_problem("", "").is_none());
        assert!(tls_pair_problem("/c.pem", "/k.pem").is_none());
    }

    /// A resolver backed by a freshly generated CA in a temporary directory.
    #[cfg(feature = "proxy-tls")]
    fn test_resolver(tld: &str) -> (SniCertResolver, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("ca.pem");
        let key = dir.path().join("ca-key.pem");
        generate_ca(&cert, &key).unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        (
            SniCertResolver::new(&cert, &key, tld.to_string()).unwrap(),
            dir,
        )
    }

    /// SANs on a certificate the resolver minted for `domain`.
    #[cfg(feature = "proxy-tls")]
    fn sans_for(resolver: &SniCertResolver, domain: &str) -> Vec<String> {
        let ck = resolver.get_or_create(domain).expect("a certificate");
        let (_, cert) = x509_parser::parse_x509_certificate(&ck.cert[0]).unwrap();
        cert.subject_alternative_name()
            .unwrap()
            .map(|ext| {
                ext.value
                    .general_names
                    .iter()
                    .filter_map(|n| match n {
                        x509_parser::extensions::GeneralName::DNSName(d) => Some(d.to_string()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn the_ca_refuses_to_sign_for_a_name_outside_the_tld() {
        use rustls::server::ResolvesServerCert;

        let (resolver, dir) = test_resolver("localhost");
        let cache = dir.path().join("host-certs");

        // The CA is installed in the machine's trust store and the proxy
        // answers on loopback, so minting for an arbitrary SNI would hand out a
        // browser-trusted certificate for somebody else's name.
        for foreign in [
            "login.microsoftonline.com",
            "example.com",
            "notlocalhost",
            "localhost.evil.com",
        ] {
            assert!(
                resolver.get_or_create_checked(foreign).is_none(),
                "expected {foreign:?} to be refused"
            );
        }

        // And nothing was written to the on-disk cache for them.
        let cached: Vec<String> = std::fs::read_dir(&cache)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            cached.is_empty(),
            "unexpected cached certificates: {cached:?}"
        );

        // Names under the TLD are still served, at any depth.
        for ours in ["localhost", "api.localhost", "core.wt.proj.localhost"] {
            assert!(
                resolver.get_or_create_checked(ours).is_some(),
                "expected {ours:?} to be issued"
            );
        }
        // `resolve` is the trait entry point and applies the same rule.
        let _ = &resolver as &dyn ResolvesServerCert;
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn the_certificate_cache_is_bounded_and_evicts_the_oldest() {
        // The set of names under a TLD is unbounded, and each one costs a key
        // generation and a file, so the cache must not grow with it.
        let (resolver, dir) = test_resolver("localhost");
        let host_certs = dir.path().join("host-certs");

        for i in 0..MAX_HOST_CERTS + 8 {
            assert!(
                resolver
                    .get_or_create_checked(&format!("h{i}.localhost"))
                    .is_some()
            );
        }

        let cached = resolver.cache.lock().unwrap();
        assert_eq!(cached.by_domain.len(), MAX_HOST_CERTS);
        assert_eq!(cached.order.len(), MAX_HOST_CERTS);
        // Oldest first out, newest retained.
        assert!(cached.get("h0.localhost").is_none());
        assert!(
            cached
                .get(&format!("h{}.localhost", MAX_HOST_CERTS + 7))
                .is_some()
        );
        drop(cached);

        // The files went with them, so the disk cache is bounded too.
        let on_disk = std::fs::read_dir(&host_certs).unwrap().count();
        assert!(
            on_disk <= MAX_HOST_CERTS,
            "{on_disk} files cached, expected at most {MAX_HOST_CERTS}"
        );
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn the_disk_cache_is_pruned_at_startup() {
        // Eviction during a run only knows the names that run has seen, so
        // files left by earlier processes have to be cleared on the way in or
        // the directory grows across restarts.
        let dir = tempfile::tempdir().unwrap();
        let host_certs = dir.path().join("host-certs");
        std::fs::create_dir_all(&host_certs).unwrap();
        for i in 0..MAX_HOST_CERTS + 20 {
            std::fs::write(host_certs.join(format!("old{i}.pem")), "stale").unwrap();
        }
        // A file we do not own is left alone.
        std::fs::write(host_certs.join("notes.txt"), "keep me").unwrap();

        prune_host_certs(&host_certs);

        let pems = std::fs::read_dir(&host_certs)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "pem"))
            .count();
        assert_eq!(pems, MAX_HOST_CERTS);
        assert!(host_certs.join("notes.txt").exists());

        // Under the limit, nothing is touched.
        let small = dir.path().join("small");
        std::fs::create_dir_all(&small).unwrap();
        std::fs::write(small.join("a.pem"), "x").unwrap();
        prune_host_certs(&small);
        assert!(small.join("a.pem").exists());
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn a_minted_certificate_never_wildcards_the_whole_tld() {
        let (resolver, _dir) = test_resolver("localhost");

        // One level down: the parent is the TLD itself, so no wildcard.
        let sans = sans_for(&resolver, "api.localhost");
        assert!(sans.contains(&"api.localhost".to_string()));
        assert!(
            !sans.iter().any(|s| s.starts_with('*')),
            "unexpected wildcard in {sans:?}"
        );

        // Deeper: the sibling wildcard is inside the TLD, which is fine.
        let sans = sans_for(&resolver, "core.wt.proj.localhost");
        assert!(sans.contains(&"*.wt.proj.localhost".to_string()));

        // A multi-label TLD is still a TLD; `*.dev.internal` would cover it all.
        let (resolver, _dir) = test_resolver("dev.internal");
        let sans = sans_for(&resolver, "api.dev.internal");
        assert!(
            !sans.iter().any(|s| s.starts_with('*')),
            "unexpected wildcard in {sans:?}"
        );
    }

    /// The placeholder has to say why there is no page: the web UI being off is
    /// a different problem from a checkout no registered project covers, and
    /// the advice differs.
    #[tokio::test]
    async fn test_page_placeholder_explains_which_step_is_missing() {
        async fn body_of(response: Response) -> String {
            let (_, body) = response.into_parts();
            let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        }

        let disabled = page_placeholder_response("shop", None, &[], "localhost", "", None);
        assert_eq!(disabled.status(), StatusCode::OK);
        let disabled = body_of(disabled).await;
        assert!(disabled.contains("auto_start"), "{disabled}");
        assert!(!disabled.contains("[namespaces]"));

        let running = page_placeholder_response(
            "shop",
            Some("feature-a"),
            &[],
            "localhost",
            "",
            Some("http://127.0.0.1:3120"),
        );
        let running = body_of(running).await;
        assert!(running.contains("[namespaces]"), "{running}");
        assert!(running.contains("http://127.0.0.1:3120/projects"));
        assert!(!running.contains("auto_start"));
    }

    /// A reserved project or stack hostname sends the browser to the page in
    /// the web UI, which is where those pages live.
    #[test]
    fn test_page_redirect_targets_the_web_ui() {
        let response = page_redirect_response("http://127.0.0.1:3120", "/projects/shop/feature-a");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .unwrap(),
            "http://127.0.0.1:3120/projects/shop/feature-a"
        );

        let project = page_redirect_response("http://127.0.0.1:3120/ps", "/projects/shop");
        assert_eq!(
            project.headers().get(axum::http::header::LOCATION).unwrap(),
            // The base carries the web UI's path prefix when one is set.
            "http://127.0.0.1:3120/ps/projects/shop"
        );
    }

    #[test]
    fn test_strip_tld() {
        assert_eq!(
            strip_tld("api.myproject.localhost", "localhost"),
            Some("api.myproject".to_string())
        );
        // Host names are case-insensitive, and browsers pass on what was typed.
        assert_eq!(
            strip_tld("API.MyProject.LOCALHOST", "localhost"),
            Some("API.MyProject".to_string())
        );
        assert_eq!(
            strip_tld("api.localhost", "LOCALHOST"),
            Some("api".to_string())
        );
        assert_eq!(
            strip_tld("api.localhost", "localhost"),
            Some("api".to_string())
        );
        assert_eq!(strip_tld("localhost", "localhost"), None);
        // Host names are case-insensitive, and a trailing root dot names the
        // same host. rustls hands over the SNI name as the client wrote it, so
        // both spellings have to resolve or a passthrough hostname would be
        // read as terminating.
        assert_eq!(
            strip_tld("API.LocalHost", "localhost"),
            Some("API".to_string())
        );
        assert_eq!(
            strip_tld("api.localhost.", "localhost"),
            Some("api".to_string())
        );
        assert_eq!(
            strip_tld("API.MyProject.LOCALHOST.", "localhost"),
            Some("API.MyProject".to_string())
        );
        assert_eq!(strip_tld("localhost.", "localhost"), None);
        assert_eq!(
            strip_tld("api.myproject.test", "test"),
            Some("api.myproject".to_string())
        );
        assert_eq!(strip_tld("other.com", "localhost"), None);
    }

    fn make_entry(name: &str) -> CachedSlugEntry {
        CachedSlugEntry {
            slug: name.to_string(),
            namespace: None,
            daemon_name: name.to_string(),
            dir: std::path::PathBuf::from(format!("/tmp/{name}")),
            worktrees: vec![],
            rejected_worktree_prefixes: std::collections::HashSet::new(),
            tls: ProxyTlsRoute::default(),
            worktree_tls: std::collections::HashMap::new(),
        }
    }

    /// A stopped daemon record with the given configured and resolved ports.
    fn make_daemon(
        configured: &[u16],
        resolved: &[u16],
        active: Option<u16>,
    ) -> crate::daemon::Daemon {
        crate::daemon::Daemon {
            id: DaemonId::try_new("proj", "api").unwrap(),
            port: crate::config_types::PortConfig::from_parts(
                configured.to_vec(),
                crate::config_types::PortBump(0),
            ),
            resolved_port: resolved.to_vec(),
            active_port: active,
            ..crate::daemon::Daemon::default()
        }
    }

    /// Without `proxy_tls_port`, the detected listening port wins, exactly as
    /// before this setting existed.
    #[test]
    fn test_select_daemon_port_prefers_active_port() {
        let route = ProxyTlsRoute::default();
        let d = make_daemon(&[8443, 9443], &[8443, 9443], Some(8443));
        assert_eq!(select_daemon_port(&route, &d), Some(8443));
    }

    /// A port of 0 is not connectable — it is what a daemon carries when it
    /// asked the operating system to choose and nothing has been detected yet
    /// — so it is skipped rather than spliced to.
    #[test]
    fn test_select_daemon_port_skips_port_zero() {
        for mode in [ProxyTlsMode::Passthrough, ProxyTlsMode::Terminate] {
            let route = ProxyTlsRoute { mode, port: None };

            // A later real port is used in place of the placeholder.
            let mixed = make_daemon(&[0, 8443], &[0, 8443], None);
            assert_eq!(select_daemon_port(&route, &mixed), Some(8443));

            // A placeholder in the detected port must not shadow a real one
            // further down: it is skipped, not treated as the answer.
            let detected_placeholder = make_daemon(&[0, 8443], &[0, 8443], Some(0));
            assert_eq!(
                select_daemon_port(&route, &detected_placeholder),
                Some(8443)
            );

            // With nothing but placeholders there is no route.
            let unresolved = make_daemon(&[0], &[0], Some(0));
            assert_eq!(select_daemon_port(&route, &unresolved), None);
        }
    }

    /// A passthrough hostname without `proxy_tls_port` takes the daemon's
    /// declared first port rather than whichever listener was detected first:
    /// on a multi-port daemon the detected one can be a secondary plain-HTTP
    /// listener that came up before the TLS one.
    #[test]
    fn test_select_daemon_port_passthrough_prefers_declared_first_port() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: None,
        };
        let d = make_daemon(&[8443, 9080], &[8443, 9080], Some(9080));
        assert_eq!(select_daemon_port(&route, &d), Some(8443));

        // With nothing resolved, the detected port is still better than
        // refusing to route.
        let detected_only = make_daemon(&[], &[], Some(9080));
        assert_eq!(select_daemon_port(&route, &detected_only), Some(9080));
    }

    /// With no detected port yet, the first resolved port is used.
    #[test]
    fn test_select_daemon_port_falls_back_to_first_resolved() {
        let route = ProxyTlsRoute::default();
        let d = make_daemon(&[8443, 9443], &[8443, 9443], None);
        assert_eq!(select_daemon_port(&route, &d), Some(8443));

        // A daemon with no ports at all does not route.
        let none = make_daemon(&[], &[], None);
        assert_eq!(select_daemon_port(&route, &none), None);
    }

    /// `proxy_tls_port` picks a later port of a multi-port daemon, overriding
    /// the detected first port.
    #[test]
    fn test_select_daemon_port_honors_configured_port() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(9443),
        };
        let d = make_daemon(&[8443, 9443], &[8443, 9443], Some(8443));
        assert_eq!(select_daemon_port(&route, &d), Some(9443));
    }

    /// Auto-bump shifts the ports a daemon actually binds. The hostname still
    /// maps to the same *position* in its port list, so it follows the bump
    /// instead of pointing at a port nothing is listening on.
    #[test]
    fn test_select_daemon_port_follows_auto_bump() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(9443),
        };
        let d = make_daemon(&[8443, 9443], &[8444, 9444], Some(8444));
        assert_eq!(select_daemon_port(&route, &d), Some(9444));
    }

    /// A configured `proxy_tls_port` whose resolved port is still the
    /// placeholder 0 does not route either: the position matches, but nothing
    /// is listening there yet.
    #[test]
    fn test_select_daemon_port_skips_a_configured_port_resolved_to_zero() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(9443),
        };
        // Declared [8443, 9443], but the second slot has not been resolved to
        // a real port yet.
        let pending = make_daemon(&[8443, 9443], &[8443, 0], Some(8443));
        assert_eq!(select_daemon_port(&route, &pending), None);

        // Once it resolves, the same hostname routes to it.
        let ready = make_daemon(&[8443, 9443], &[8443, 9444], Some(8443));
        assert_eq!(select_daemon_port(&route, &ready), Some(9444));
    }

    /// A daemon whose recorded ports contradict `proxy_tls_port` — a state
    /// record left by an older config — is not routed to a port it never
    /// bound. Config validation stops this combination from being written in
    /// the first place.
    #[test]
    fn test_select_daemon_port_refuses_a_port_the_daemon_never_bound() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(9443),
        };
        let stale = make_daemon(&[8443], &[8443], Some(8443));
        assert_eq!(select_daemon_port(&route, &stale), None);

        // With nothing recorded to contradict it, the configured port is all
        // the information there is, so it is used.
        let bare = make_daemon(&[], &[], None);
        assert_eq!(select_daemon_port(&route, &bare), Some(9443));
    }

    /// `proxy_tls_port` selects the forwarded port in terminate mode too, not
    /// only for passthrough hostnames.
    #[test]
    fn test_select_daemon_port_honors_configured_port_when_terminating() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Terminate,
            port: Some(9080),
        };
        let d = make_daemon(&[8080, 9080], &[8080, 9080], Some(8080));
        assert_eq!(select_daemon_port(&route, &d), Some(9080));

        // Following auto-bump by position, as in passthrough mode.
        let bumped = make_daemon(&[8080, 9080], &[8081, 9081], Some(8081));
        assert_eq!(select_daemon_port(&route, &bumped), Some(9081));
    }

    /// A directory that says nothing about the daemon reads as no route at
    /// all, so a worktree the proxy cannot read inherits its slug's mode
    /// instead of being recorded as terminating and quietly ending a splice.
    #[test]
    fn test_read_proxy_tls_route_absent_without_config() {
        let dir = tempfile::tempdir().unwrap();

        // A directory with no config at all.
        assert_eq!(
            read_proxy_tls_route(dir.path(), Some("proj"), "api").unwrap(),
            None
        );

        // A config that describes some other daemon.
        std::fs::write(
            dir.path().join("pitchfork.toml"),
            "[daemons.other]\nrun = \"serve\"\n",
        )
        .unwrap();
        assert_eq!(
            read_proxy_tls_route(dir.path(), Some("proj"), "api").unwrap(),
            None,
            "a config without this daemon says nothing about it"
        );

        // No namespace to resolve the daemon against.
        assert_eq!(read_proxy_tls_route(dir.path(), None, "api").unwrap(), None);
    }

    /// A config that cannot be read — here because an unrelated daemon is
    /// invalid — is an error, not a route of `terminate`, and a refresh keeps
    /// the route it already knew instead of downgrading a passthrough daemon.
    #[test]
    fn test_unreadable_config_keeps_the_last_known_route() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pitchfork.toml"),
            "[daemons.api]\nrun = \"serve\"\nport = 8443\nproxy_tls = \"passthrough\"\n\n\
             [daemons.broken]\nrun = \"serve\"\nproxy_tls = \"passthrough\"\n",
        )
        .unwrap();
        let read = read_proxy_tls_route(dir.path(), Some("proj"), "api");
        assert!(
            read.is_err(),
            "an invalid sibling makes the config unreadable"
        );

        let known = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(8443),
        };
        assert_eq!(
            route_or_last_known(read, Some(known), dir.path(), "api"),
            Some(known)
        );

        // A readable answer replaces what was known, even when it is silence.
        assert_eq!(
            route_or_last_known(Ok(None), Some(known), dir.path(), "api"),
            None
        );
    }

    /// A route is only carried over while the slug still points at the same
    /// daemon, directory and namespace; a repointed slug starts from nothing.
    #[test]
    fn test_known_route_requires_the_same_target() {
        let known = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(8443),
        };
        let mut entry = make_entry("api");
        entry.namespace = Some("proj".to_string());
        entry.tls = known;
        let dir = entry.dir.clone();

        assert_eq!(entry.known_route(&dir, Some("proj"), "api"), Some(known));
        assert_eq!(entry.known_route(&dir, Some("proj"), "web"), None);
        assert_eq!(entry.known_route(&dir, Some("other"), "api"), None);
        assert_eq!(
            entry.known_route(std::path::Path::new("/elsewhere"), Some("proj"), "api"),
            None
        );

        let wt = make_worktree("feature/x", "feature-x");
        entry.worktrees = vec![wt.clone()];
        entry.worktree_tls.insert("feature-x".to_string(), known);
        assert_eq!(entry.known_worktree_route(&wt, "api"), Some(known));
        assert_eq!(entry.known_worktree_route(&wt, "web"), None);
        let moved = crate::proxy::worktree::WorktreeEntry {
            path: std::path::PathBuf::from("/elsewhere/feature-x"),
            ..wt
        };
        assert_eq!(entry.known_worktree_route(&moved, "api"), None);
    }

    /// A worktree whose own config describes the daemon is authoritative,
    /// including when it leaves `proxy_tls` out, and one the proxy knows
    /// nothing about inherits the slug's route.
    #[test]
    fn test_worktree_route_inherits_when_unknown() {
        let mut entry = make_entry("spliced");
        entry.tls = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(8443),
        };
        entry.worktrees = vec![
            make_worktree("feature/known", "feature-known"),
            make_worktree("feature/unknown", "feature-unknown"),
        ];
        // Only the worktree whose config was readable gets an entry.
        entry.worktree_tls.insert(
            "feature-known".to_string(),
            ProxyTlsRoute {
                mode: ProxyTlsMode::Terminate,
                port: None,
            },
        );
        let mut entries = std::collections::HashMap::new();
        entries.insert("spliced".to_string(), entry);

        let mode =
            |host: &str| resolve_tls_mode_in(host, "localhost", &entries, &Default::default());
        assert_eq!(
            mode("feature-known.spliced.localhost"),
            ProxyTlsMode::Terminate,
            "an explicit worktree setting wins"
        );
        assert_eq!(
            mode("feature-unknown.spliced.localhost"),
            ProxyTlsMode::Passthrough,
            "a worktree with nothing recorded inherits the slug"
        );

        // The port is not inherited with the mode: it names a position in one
        // daemon's port list, and a worktree hostname reaches a different
        // process with its own ports.
        let cached = entries.get("spliced").unwrap();
        assert_eq!(
            worktree_route(cached, "feature-unknown"),
            ProxyTlsRoute {
                mode: ProxyTlsMode::Passthrough,
                port: None,
            }
        );
        // An explicit worktree route is taken whole, port included.
        assert_eq!(
            worktree_route(cached, "feature-known"),
            ProxyTlsRoute {
                mode: ProxyTlsMode::Terminate,
                port: None,
            }
        );
    }

    /// The route of a wildcard worktree host comes from that worktree's own
    /// config, and falls back to the slug's when the worktree has none.
    #[test]
    fn test_worktree_route_lookup() {
        let mut entry = make_entry("myapp");
        entry.tls = ProxyTlsRoute {
            mode: ProxyTlsMode::Terminate,
            port: None,
        };
        entry.worktrees = vec![
            make_worktree("feature/b", "feature-b"),
            make_worktree("feature/c", "feature-c"),
        ];
        entry.worktree_tls.insert(
            "feature-b".to_string(),
            ProxyTlsRoute {
                mode: ProxyTlsMode::Passthrough,
                port: Some(9443),
            },
        );

        let lookup = |prefix: &str| match match_worktree_prefix(&entry, prefix) {
            PrefixMatch::Worktree(wt) => entry
                .worktree_tls
                .get(&wt.sanitized_branch.to_ascii_lowercase())
                .copied()
                .unwrap_or(entry.tls),
            _ => entry.tls,
        };

        assert_eq!(lookup("feature-b").mode, ProxyTlsMode::Passthrough);
        assert_eq!(lookup("feature-b").port, Some(9443));
        // A worktree without its own setting inherits the slug's mode.
        assert_eq!(lookup("feature-c").mode, ProxyTlsMode::Terminate);
        // So does an ordinary wildcard subdomain.
        assert_eq!(lookup("tenant").mode, ProxyTlsMode::Terminate);
    }

    #[test]
    fn test_wildcard_slug_lookup_exact_match() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // Exact match takes priority.
        let result = wildcard_slug_lookup("myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "myapp");
    }

    #[test]
    fn test_wildcard_slug_lookup_subdomain_fallback() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // "tenant.myapp" falls back to "myapp".
        let result = wildcard_slug_lookup("tenant.myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "myapp");
    }

    #[test]
    fn test_wildcard_slug_lookup_nested_fallback() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // "a.b.myapp" falls back to "myapp" through "b.myapp" → "myapp".
        let result = wildcard_slug_lookup("a.b.myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "myapp");
    }

    #[test]
    fn test_wildcard_slug_lookup_no_match() {
        let entries = std::collections::HashMap::new();
        // Empty entries → no match.
        let result = wildcard_slug_lookup("tenant.myapp", &entries, true);
        assert!(result.is_none());
    }

    #[test]
    fn test_wildcard_slug_lookup_disabled() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // With wildcard disabled, "tenant.myapp" does NOT match "myapp".
        let result = wildcard_slug_lookup("tenant.myapp", &entries, false);
        assert!(result.is_none());
        // But exact match still works.
        let result = wildcard_slug_lookup("myapp", &entries, false);
        assert!(result.is_some());
    }

    #[test]
    fn test_wildcard_slug_lookup_exact_beats_wildcard() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        let mut tenant_entry = make_entry("tenant-daemon");
        tenant_entry.slug = "tenant.myapp".to_string();
        entries.insert("tenant.myapp".to_string(), tenant_entry);
        // "tenant.myapp" should match the exact slug, not fall back to "myapp".
        let result = wildcard_slug_lookup("tenant.myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "tenant-daemon");
    }

    #[test]
    fn test_wildcard_slug_lookup_ignores_case() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // Browsers lowercase the Host header, so every spelling must resolve.
        for host in ["MyApp", "MYAPP", "myapp"] {
            let result = wildcard_slug_lookup(host, &entries, true);
            assert!(result.is_some(), "exact lookup failed for {host}");
            assert_eq!(result.unwrap().daemon_name, "myapp");
        }
        // ...including through the wildcard fallback.
        for host in ["Tenant.MyApp", "tenant.MYAPP", "A.B.MyApp"] {
            let result = wildcard_slug_lookup(host, &entries, true);
            assert!(result.is_some(), "wildcard lookup failed for {host}");
            assert_eq!(result.unwrap().daemon_name, "myapp");
        }
    }

    #[test]
    fn test_wildcard_slug_lookup_case_insensitive_registration() {
        let mut entries = std::collections::HashMap::new();
        let mut entry = make_entry("upper");
        entry.slug = "MyApp".to_string();
        // build_slug_entries lowercases the key while keeping the configured
        // spelling in `slug`, so a capitalized registration stays reachable.
        entries.insert("myapp".to_string(), entry);
        for host in ["myapp", "MyApp", "tenant.MYAPP"] {
            let result = wildcard_slug_lookup(host, &entries, true);
            assert!(result.is_some(), "lookup failed for {host}");
            assert_eq!(result.unwrap().daemon_name, "upper");
        }
    }

    fn make_worktree(branch: &str, sanitized: &str) -> crate::proxy::worktree::WorktreeEntry {
        crate::proxy::worktree::WorktreeEntry {
            path: std::path::PathBuf::from(format!("/tmp/{sanitized}")),
            branch: branch.to_string(),
            sanitized_branch: sanitized.to_string(),
            namespace: Some(sanitized.to_string()),
        }
    }

    #[test]
    fn test_reject_case_colliding_worktrees_drops_both_sides() {
        let wts = vec![
            make_worktree("Feature-A", "Feature-A"),
            make_worktree("feature-a", "feature-a"),
            make_worktree("main", "main"),
        ];
        let (kept, rejected) = reject_case_colliding_worktrees(wts);
        // Neither spelling routes; picking one would send the request to a
        // worktree the user did not name.
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].sanitized_branch, "main");
        // The prefix is remembered so it is refused rather than treated as an
        // unknown wildcard prefix.
        assert!(rejected.contains("feature-a"));
    }

    #[test]
    fn test_reject_case_colliding_worktrees_keeps_unambiguous() {
        let wts = vec![
            make_worktree("main", "main"),
            make_worktree("feature/a", "feature-a"),
        ];
        let (kept, rejected) = reject_case_colliding_worktrees(wts);
        assert_eq!(kept.len(), 2);
        assert!(rejected.is_empty());
    }

    #[test]
    fn test_reject_case_colliding_worktrees_drops_sanitize_duplicates() {
        // Distinct branches can sanitize to the same string without any case
        // difference; that is ambiguous for the same reason.
        let wts = vec![
            make_worktree("feature/a", "feature-a"),
            make_worktree("feature.a", "feature-a"),
        ];
        let (kept, rejected) = reject_case_colliding_worktrees(wts);
        assert!(kept.is_empty());
        assert!(rejected.contains("feature-a"));
    }

    #[test]
    fn test_match_worktree_prefix() {
        let mut entry = make_entry("myapp");
        entry.worktrees = vec![make_worktree("feature/b", "feature-b")];
        entry
            .rejected_worktree_prefixes
            .insert("feature-a".to_string());

        assert!(matches!(
            match_worktree_prefix(&entry, "feature-b"),
            PrefixMatch::Worktree(_)
        ));
        // Host case does not matter for either outcome.
        assert!(matches!(
            match_worktree_prefix(&entry, "Feature-B"),
            PrefixMatch::Worktree(_)
        ));
        // A rejected prefix is refused, not served by the main checkout.
        assert!(matches!(
            match_worktree_prefix(&entry, "feature-a"),
            PrefixMatch::Ambiguous
        ));
        assert!(matches!(
            match_worktree_prefix(&entry, "FEATURE-A"),
            PrefixMatch::Ambiguous
        ));
        // An unrelated prefix is still an ordinary wildcard subdomain.
        assert!(matches!(
            match_worktree_prefix(&entry, "tenant"),
            PrefixMatch::Unknown
        ));
    }

    /// Each way of reaching the HTTP path with a passthrough hostname names its
    /// own remedy, since the fixes are different.
    #[test]
    fn test_passthrough_unroutable_message() {
        let no_tls = passthrough_unroutable_message("api.localhost", false);
        assert!(no_tls.contains("api.localhost"), "{no_tls}");
        assert!(no_tls.contains("settings.proxy.https = true"), "{no_tls}");

        // Over TLS this branch is reached when the handshake named a
        // different host than the request did — a connection with no host
        // name at all fails in the certificate resolver and never gets here.
        let mismatch = passthrough_unroutable_message("api.localhost", true);
        assert!(mismatch.contains("named a different host"), "{mismatch}");
        assert!(
            !mismatch.contains("settings.proxy.https"),
            "a host mismatch is not an HTTPS configuration problem: {mismatch}"
        );
    }

    /// Routing and the certificate resolver read one table, so they cannot
    /// disagree about a hostname's mode: the synchronous read returns the very
    /// same allocation the async one does.
    #[tokio::test]
    async fn test_slug_snapshot_is_the_cached_table() {
        let from_async = get_cached_slugs().await;
        let from_sync = slug_snapshot();
        assert!(
            Arc::ptr_eq(&from_async, &from_sync),
            "the synchronous read must see the same table routing does"
        );
    }

    /// Callers arriving together on an expired cache share one table rather
    /// than each building and holding its own. A caller holding its own build
    /// would route a hostname by a table the certificate resolver does not
    /// read.
    #[tokio::test]
    async fn test_concurrent_refresh_returns_the_published_table() {
        // The cache starts expired, so both calls take the refresh path; the
        // second waits for the first rather than building a second table.
        let (first, second) = tokio::join!(get_cached_slugs(), get_cached_slugs());
        assert!(
            Arc::ptr_eq(&first, &second),
            "overlapping refreshes must agree on one table"
        );
        assert!(
            Arc::ptr_eq(&first, &slug_snapshot()),
            "and it must be the published one"
        );
    }

    /// The synchronous mode lookup the certificate resolver uses agrees with
    /// the async one: exact hosts, wildcard subdomains, worktree prefixes with
    /// their own setting, and anything that is not a slug at all.
    #[test]
    fn test_resolve_tls_mode_in() {
        let mut entries = std::collections::HashMap::new();

        let mut spliced = make_entry("spliced");
        spliced.tls = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(8443),
        };
        spliced.worktrees = vec![
            make_worktree("feature/b", "feature-b"),
            make_worktree("feature/c", "feature-c"),
        ];
        spliced.worktree_tls.insert(
            "feature-b".to_string(),
            ProxyTlsRoute {
                mode: ProxyTlsMode::Terminate,
                port: None,
            },
        );
        entries.insert("spliced".to_string(), spliced);
        entries.insert("plain".to_string(), make_entry("plain"));

        let mode =
            |host: &str| resolve_tls_mode_in(host, "localhost", &entries, &Default::default());

        assert_eq!(mode("spliced.localhost"), ProxyTlsMode::Passthrough);
        // Host names are case-insensitive, including the TLD, and a trailing
        // root dot names the same host — rustls passes the SNI name through
        // exactly as the client wrote it.
        assert_eq!(mode("SPLICED.localhost"), ProxyTlsMode::Passthrough);
        assert_eq!(mode("Spliced.LocalHost"), ProxyTlsMode::Passthrough);
        assert_eq!(mode("spliced.localhost."), ProxyTlsMode::Passthrough);
        assert_eq!(
            mode("FEATURE-C.Spliced.LOCALHOST."),
            ProxyTlsMode::Passthrough
        );
        // A wildcard subdomain inherits the slug's mode.
        assert_eq!(mode("tenant.spliced.localhost"), ProxyTlsMode::Passthrough);
        // A worktree with its own setting overrides the slug's …
        assert_eq!(mode("feature-b.spliced.localhost"), ProxyTlsMode::Terminate);
        // … and one without it inherits.
        assert_eq!(
            mode("feature-c.spliced.localhost"),
            ProxyTlsMode::Passthrough
        );
        // Everything else terminates: another slug, an unknown host, the
        // bare TLD, and a host outside the TLD.
        assert_eq!(mode("plain.localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("unknown.localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("spliced.example.com"), ProxyTlsMode::Terminate);
    }

    /// A daemon reached through its automatic `<daemon>.<project>` hostname
    /// gets the mode from its own config, the same as one reached through a
    /// legacy slug. Automatic hostnames are the common case, so passthrough
    /// has to work without a slug registration.
    #[test]
    fn test_resolve_tls_mode_in_uses_the_hostname_registry() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("autoproj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("pitchfork.toml"),
            "[daemons.secure]\nrun = \"serve\"\nport = 8443\nproxy_tls = \"passthrough\"\n\
             [daemons.plain]\nrun = \"serve\"\nport = 8080\n",
        )
        .unwrap();

        let registry =
            crate::proxy::hostname::HostRegistry::from_dirs(std::slice::from_ref(&project));
        let slugs = std::collections::HashMap::new();
        let mode = |host: &str| resolve_tls_mode_in(host, "localhost", &slugs, &registry);

        assert_eq!(mode("secure.autoproj.localhost"), ProxyTlsMode::Passthrough);
        assert_eq!(mode("plain.autoproj.localhost"), ProxyTlsMode::Terminate);

        // The mode travels with the registry entry, so a config that becomes
        // unreadable after the registry was built cannot turn the passthrough
        // daemon into a terminated one.
        std::fs::write(project.join("pitchfork.toml"), "this is not toml = [\n").unwrap();
        assert_eq!(mode("secure.autoproj.localhost"), ProxyTlsMode::Passthrough);
        // The project page is served by the proxy itself, over its own
        // certificate, as is a name nothing claims.
        assert_eq!(mode("autoproj.localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("nothing.autoproj.localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("unknown.localhost"), ProxyTlsMode::Terminate);
    }

    /// An empty table — the snapshot before any refresh — terminates rather
    /// than refusing certificates for hosts it knows nothing about.
    #[test]
    fn test_resolve_tls_mode_in_empty_table() {
        let entries = std::collections::HashMap::new();
        assert_eq!(
            resolve_tls_mode_in(
                "spliced.localhost",
                "localhost",
                &entries,
                &Default::default()
            ),
            ProxyTlsMode::Terminate
        );
    }

    #[test]
    fn test_strip_dot_suffix_ignore_case() {
        assert_eq!(
            strip_dot_suffix_ignore_case("feature-a.myapp", "myapp"),
            Some("feature-a".to_string())
        );
        assert_eq!(
            strip_dot_suffix_ignore_case("Feature-A.MyApp", "myapp"),
            Some("Feature-A".to_string())
        );
        assert_eq!(
            strip_dot_suffix_ignore_case("feature-a.myapp", "MYAPP"),
            Some("feature-a".to_string())
        );
        // No dot separator, no prefix left, and a non-matching suffix all fail.
        assert_eq!(strip_dot_suffix_ignore_case("xmyapp", "myapp"), None);
        assert_eq!(strip_dot_suffix_ignore_case(".myapp", "myapp"), None);
        assert_eq!(strip_dot_suffix_ignore_case("myapp", "myapp"), None);
        assert_eq!(
            strip_dot_suffix_ignore_case("feature-a.other", "myapp"),
            None
        );
        // Multi-byte input must not panic on a mid-character split.
        assert_eq!(
            strip_dot_suffix_ignore_case("café.myapp", "myapp"),
            Some("café".to_string())
        );
        assert_eq!(strip_dot_suffix_ignore_case("café", "afé"), None);
    }

    /// Build a minimal ClientHello naming `host`, as it goes on the wire.
    #[cfg(feature = "proxy-tls")]
    fn client_hello_wire(host: &str) -> Vec<u8> {
        let mut entry = vec![0u8];
        entry.extend_from_slice(&(host.len() as u16).to_be_bytes());
        entry.extend_from_slice(host.as_bytes());
        let mut sni = (entry.len() as u16).to_be_bytes().to_vec();
        sni.extend_from_slice(&entry);

        let mut ext = vec![0x00, 0x00];
        ext.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        ext.extend_from_slice(&sni);

        let mut body = vec![0x03, 0x03];
        body.extend_from_slice(&[0x22; 32]);
        body.push(0);
        body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]);
        body.extend_from_slice(&[0x01, 0x00]);
        body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
        body.extend_from_slice(&ext);

        let mut msg = vec![0x01];
        let len = body.len() as u32;
        msg.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        msg.extend_from_slice(&body);

        let mut record = vec![0x16, 0x03, 0x01];
        record.extend_from_slice(&(msg.len() as u16).to_be_bytes());
        record.extend_from_slice(&msg);
        record
    }

    /// Accept one connection, hand the peeked verdict back, and report what a
    /// reader sees afterwards.
    #[cfg(feature = "proxy-tls")]
    async fn probe_over_socket(
        writes: Vec<Vec<u8>>,
        gap: std::time::Duration,
        timeout: std::time::Duration,
    ) -> (SniProbe, Vec<u8>) {
        probe_over_socket_then(writes, gap, timeout, false).await
    }

    /// [`probe_over_socket`], optionally shutting down the client's write side
    /// once everything is sent.
    #[cfg(feature = "proxy-tls")]
    async fn probe_over_socket_then(
        writes: Vec<Vec<u8>>,
        gap: std::time::Duration,
        timeout: std::time::Duration,
        close_after: bool,
    ) -> (SniProbe, Vec<u8>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let total: usize = writes.iter().map(Vec::len).sum();

        let client = tokio::spawn(async move {
            let mut sock = TcpStream::connect(addr).await.unwrap();
            for chunk in writes {
                sock.write_all(&chunk).await.unwrap();
                sock.flush().await.unwrap();
                tokio::time::sleep(gap).await;
            }
            if close_after {
                sock.shutdown().await.unwrap();
            }
            // Hold the connection open so the server can read back what it
            // only peeked at.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        });

        let (stream, _) = listener.accept().await.unwrap();
        let probe = peek_sni_host(&stream, timeout).await;

        // Whatever was peeked must still be readable, byte for byte.
        let mut replayed = vec![0u8; total];
        let mut stream = stream;
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream.read_exact(&mut replayed),
        )
        .await;
        let replayed = match read {
            Ok(Ok(_)) => replayed,
            _ => vec![],
        };
        client.abort();
        (probe, replayed)
    }

    /// The peek loop reassembles a hello that arrives in many small writes,
    /// and leaves every byte in the socket for the path that handles the
    /// connection.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_reads_a_hello_split_across_writes() {
        let wire = client_hello_wire("api.localhost");
        let writes: Vec<Vec<u8>> = wire.chunks(3).map(<[u8]>::to_vec).collect();
        let (probe, replayed) = probe_over_socket(
            writes,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_secs(5),
        )
        .await;

        assert_eq!(probe, SniProbe::Host("api.localhost".to_string()));
        assert_eq!(replayed, wire, "peeked bytes must still be readable");
    }

    /// A hello that stops half way is reported as undetermined once the
    /// timeout expires, never as "no hostname": the caller must not terminate
    /// a connection that might belong to a passthrough daemon.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_undetermined_when_a_hello_stalls() {
        let wire = client_hello_wire("api.localhost");
        let truncated = wire[..wire.len() / 2].to_vec();
        let (probe, _) = probe_over_socket(
            vec![truncated],
            std::time::Duration::ZERO,
            std::time::Duration::from_millis(150),
        )
        .await;

        assert_eq!(probe, SniProbe::Undetermined);
    }

    /// A client that sends part of a hello and then closes is given up on as
    /// soon as the close arrives, not after the whole timeout: `peek` keeps
    /// returning the buffered bytes, so only the socket's readiness shows it.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_notices_a_client_that_closes_mid_hello() {
        let wire = client_hello_wire("api.localhost");
        let truncated = wire[..wire.len() / 2].to_vec();
        let started = std::time::Instant::now();
        let (probe, _) = probe_over_socket_then(
            vec![truncated],
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(5),
            true,
        )
        .await;

        assert_eq!(probe, SniProbe::Undetermined);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "the probe must end when the client closes, not at its deadline"
        );
    }

    /// A peer that opens a connection and then sends nothing is given up on
    /// at the deadline. Without the read itself being bounded this waits for
    /// as long as the peer keeps the socket open.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_gives_up_on_a_silent_peer() {
        let started = std::time::Instant::now();
        let (probe, _) = probe_over_socket(
            vec![],
            std::time::Duration::ZERO,
            std::time::Duration::from_millis(150),
        )
        .await;

        assert_eq!(probe, SniProbe::Undetermined);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "the probe must end at its deadline, not wait on the peer"
        );
    }

    /// Something that is not a TLS handshake is a definite "no hostname", so
    /// the connection is still served rather than dropped.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_reports_no_host_for_non_tls() {
        let (probe, _) = probe_over_socket(
            vec![b"GET / HTTP/1.1\r\n\r\n".to_vec()],
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(5),
        )
        .await;

        assert_eq!(probe, SniProbe::NoHost);
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn test_generate_ca() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("ca.pem");
        let key_path = dir.path().join("ca-key.pem");

        generate_ca(&cert_path, &key_path).unwrap();

        assert!(cert_path.exists(), "ca.pem should be created");
        assert!(key_path.exists(), "ca-key.pem should be created");

        let cert_pem = std::fs::read_to_string(&cert_path).unwrap();
        let key_pem = std::fs::read_to_string(&key_path).unwrap();

        assert!(cert_pem.contains("BEGIN CERTIFICATE"), "should be PEM cert");
        assert!(
            key_pem.contains("BEGIN") && key_pem.contains("PRIVATE KEY"),
            "should be PEM key"
        );
    }

    /// The raw `cookie` field values, in the order the map holds them.
    fn cookie_fields(headers: &HeaderMap) -> Vec<&[u8]> {
        headers
            .get_all(COOKIE)
            .iter()
            .map(HeaderValue::as_bytes)
            .collect()
    }

    /// Several fields become one, joined with `"; "`, and a comma inside a
    /// value is left alone.
    #[test]
    fn test_join_cookie_fields_joins_with_semicolon_space() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_static("_session=abc123"));
        headers.append(COOKIE, HeaderValue::from_static("consent=ads,stats"));
        headers.append(COOKIE, HeaderValue::from_static("theme=dark"));

        join_cookie_fields(&mut headers);

        assert_eq!(
            cookie_fields(&headers),
            vec![&b"_session=abc123; consent=ads,stats; theme=dark"[..]]
        );
    }

    /// A UTF-8 cookie value is joined like any other, since the join works on
    /// bytes rather than on visible ASCII.
    #[test]
    fn test_join_cookie_fields_joins_bytes_outside_ascii() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_static("_session=abc123"));
        headers.append(
            COOKIE,
            HeaderValue::from_bytes(b"name=Jos\xc3\xa9").unwrap(),
        );

        join_cookie_fields(&mut headers);

        assert_eq!(
            cookie_fields(&headers),
            vec![&b"_session=abc123; name=Jos\xc3\xa9"[..]]
        );
    }

    /// A request without cookies gains none.
    #[test]
    fn test_join_cookie_fields_without_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("app.localhost"));

        join_cookie_fields(&mut headers);

        assert!(headers.get(COOKIE).is_none());
    }

    #[test]
    fn test_host_port_suffix() {
        assert_eq!(host_port_suffix("api.myproj.localhost:8088"), ":8088");
        assert_eq!(host_port_suffix("api.myproj.localhost"), "");
        assert_eq!(host_port_suffix("[::1]:8088"), ":8088");
        assert_eq!(host_port_suffix("[::1]"), "");
        // A non-numeric tail is not a port and must not reach a link.
        assert_eq!(host_port_suffix("host:notaport"), "");
    }

    /// A daemon belongs to the checkout that contains its working directory,
    /// which is the worktree rather than the primary when one is nested inside
    /// the other, and no checkout at all when its `dir` points elsewhere.
    #[test]
    fn test_daemon_runs_in() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir_all(repo.join(".git/worktrees/feature")).unwrap();
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        // A worktree checked out *inside* the primary's directory tree.
        let nested = repo.join(".worktrees/feature");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join(".git"),
            format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/feature").display()
            ),
        )
        .unwrap();

        let root = |p: &std::path::Path| crate::proxy::hostname::checkout_root_of(p);
        let repo_root = root(&repo);
        let nested_root = root(&nested);

        let mut daemon = crate::daemon::Daemon {
            dir: Some(repo.join("sub")),
            ..Default::default()
        };
        assert!(daemon_runs_in(&daemon, &repo_root));
        assert!(!daemon_runs_in(&daemon, &nested_root));

        // Lexically the nested worktree sits under the primary; by checkout it
        // does not, so the primary's hostname must not claim it.
        daemon.dir = Some(nested.clone());
        assert!(daemon_runs_in(&daemon, &nested_root));
        assert!(!daemon_runs_in(&daemon, &repo_root));

        // An explicit dir outside every checkout belongs to none of them.
        daemon.dir = Some(temp.path().join("elsewhere"));
        assert!(!daemon_runs_in(&daemon, &repo_root));

        daemon.dir = None;
        assert!(!daemon_runs_in(&daemon, &repo_root));
    }

    /// Only a request known to come from this machine gets the detailed pages;
    /// an unknown peer counts as remote.
    #[test]
    fn test_is_local_client() {
        let build = |info: Option<SocketAddr>| {
            let mut req = Request::new(Body::empty());
            if let Some(addr) = info {
                req.extensions_mut()
                    .insert(axum::extract::ConnectInfo(addr));
            }
            req
        };

        assert!(is_local_client(&build(Some(
            "127.0.0.1:5000".parse().unwrap()
        ))));
        assert!(is_local_client(&build(Some("[::1]:5000".parse().unwrap()))));
        assert!(!is_local_client(&build(Some(
            "192.168.1.42:5000".parse().unwrap()
        ))));
        assert!(!is_local_client(&build(None)));
    }
}
