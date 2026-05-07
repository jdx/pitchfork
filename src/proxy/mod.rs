//! Reverse proxy server for pitchfork daemons.
//!
//! Routes `<slug>.<tld>:<port>` to the daemon's actual listening port.
//! Slugs are defined in the global config (`~/.config/pitchfork/config.toml`)
//! under `[slugs]`. Each slug maps to a project directory and daemon name.
//!
//! # URL Routing
//!
//! ```text
//! myapp.localhost:7777          →  localhost:8080  (via slug)
//! ```

pub mod hosts;
pub mod lan_ip;
pub mod mdns;
pub mod server;
