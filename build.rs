//! Build script for pitchfork
//!
//! Settings are declared directly on the structs in `src/settings.rs` via
//! `#[derive(usage_rs::Config)]`; nothing is generated here any more. This
//! script only tracks the embedded web UI assets.

fn main() {
    println!("cargo:rerun-if-changed=ui/dist");
}
