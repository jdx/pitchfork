mod cli;
mod env;
mod pid_file;
mod procs;

pub use eyre::Result;

fn main() -> Result<()> {
    let env = env_logger::Env::new().filter("PITCHFORK_LOG").write_style("PITCHFORK_LOG_STYLE");
    env_logger::init_from_env(env);
    cli::run()
}
