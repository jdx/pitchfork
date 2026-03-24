//! Reverse proxy server for pitchfork daemons.
//!
//! Routes `<slug>.<tld>:<port>` to the daemon's actual listening port.
//! Only daemons with an explicit `slug` are routable — no slug means not proxied.
//!
//! # URL Routing
//!
//! ```text
//! myapp.localhost:7777          →  localhost:8080  (via slug)
//! ```

pub mod server;
