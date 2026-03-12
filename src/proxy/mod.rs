//! Reverse proxy server for pitchfork daemons.
//!
//! Routes `<id>.<namespace>.<tld>:<port>` to the daemon's actual listening port.
//!
//! # URL Routing
//!
//! ```text
//! api.myproject.localhost:7777  →  localhost:3000
//! api.localhost:7777            →  localhost:3000  (global namespace)
//! myapp.localhost:7777          →  localhost:8080  (via slug)
//! ```

pub mod server;
