//! Build script for pitchfork
//!
//! Settings are declared directly on the structs in `src/settings.rs` via
//! `#[derive(usage_rs::Config)]`; nothing is generated here any more. This
//! script tracks the embedded web UI assets and, for release builds, links the
//! executable non-PIE.

fn main() {
    println!("cargo:rerun-if-changed=ui/dist");
    link_without_pie();
}

/// Release builds for Linux GNU set `PITCHFORK_NO_PIE=1` (see
/// .github/workflows/release.yml) to link the executable at a fixed address. As
/// a position-independent executable, pitchfork makes the dynamic loader patch
/// about 50k pointers on every launch, which copies hundreds of pages before
/// `main` runs. Linked non-PIE, those pointers are final in the file.
/// Dependencies are still compiled position-independent; the flag reaches only
/// bin targets, so no shared library is linked with it.
fn link_without_pie() {
    println!("cargo:rerun-if-env-changed=PITCHFORK_NO_PIE");
    let linux_gnu = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");
    if linux_gnu && std::env::var("PITCHFORK_NO_PIE").as_deref() == Ok("1") {
        println!("cargo:rustc-link-arg-bins=-no-pie");
    }
}
