//! Build script for pitchfork
//!
//! This script generates the settings module from settings.toml at compile time.

#[path = "build/generate_settings.rs"]
mod generate_settings;

fn main() {
    // Tell Cargo to rerun this script when settings.toml changes
    println!("cargo:rerun-if-changed=settings.toml");
    println!("cargo:rerun-if-changed=build/generate_settings.rs");

    // Generate the settings module
    if let Err(e) = generate_settings::generate() {
        eprintln!("Failed to generate settings: {}", e);
        std::process::exit(1);
    }
}
