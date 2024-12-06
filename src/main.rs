mod cli;
mod env;

pub use eyre::Result;

fn main() -> Result<()> {
    env_logger::init();
    cli::run()
}
