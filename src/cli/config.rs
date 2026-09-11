use crate::pitchfork_toml::PitchforkToml;
use crate::{Result, env, extra_configs};
use miette::IntoDiagnostic;
use std::path::PathBuf;

/// Attach externally generated configuration to a project.
#[derive(Debug, usage_rs::Args)]
#[usage(args_conflicts_with_subcommands = true)]
pub struct Config {
    #[usage(subcommand)]
    command: Option<Commands>,
    #[usage(flatten)]
    list: List,
}

#[derive(Debug, usage_rs::Subcommands)]
enum Commands {
    Add(Add),
    #[usage(alias = "rm")]
    Remove(Remove),
    #[usage(alias = "ls")]
    List(List),
}

/// Register a configuration file for a project without copying it into the project.
#[derive(Debug, usage_rs::Args)]
struct Add {
    file: PathBuf,
    #[usage(long)]
    dir: Option<PathBuf>,
    #[usage(long)]
    namespace: Option<String>,
}

/// Detach a configuration file, including a file that no longer exists.
#[derive(Debug, usage_rs::Args)]
struct Remove {
    file: PathBuf,
}

/// List registered and invocation-scoped configuration files.
#[derive(Debug, Default, usage_rs::Args)]
struct List {
    #[usage(long)]
    json: bool,
}

impl Config {
    pub async fn run(self) -> Result<()> {
        tokio::task::spawn_blocking(move || self.run_blocking())
            .await
            .into_diagnostic()?
    }

    fn run_blocking(&self) -> Result<()> {
        match &self.command {
            Some(Commands::Add(args)) => {
                let file = env::expand_tilde(&args.file)
                    .canonicalize()
                    .into_diagnostic()?;
                let dir = env::expand_tilde(args.dir.as_deref().unwrap_or(&env::CWD))
                    .canonicalize()
                    .into_diagnostic()?;
                if !dir.is_dir() {
                    miette::bail!("--dir must name a directory");
                }
                let namespace = args
                    .namespace
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| PitchforkToml::namespace_for_project_dir(&dir))?;
                crate::daemon_id::DaemonId::try_new(&namespace, "probe")?;
                if crate::pitchfork_toml::is_global_config(&file) {
                    miette::bail!("global configuration cannot also be attached to a project");
                }
                if let Some(actual) = PitchforkToml::project_namespace_override(&dir)?
                    && actual != namespace
                {
                    miette::bail!("project namespace '{actual}' does not match '{namespace}'");
                }
                // Validate with the registration's namespace before its mapping exists.
                let content = std::fs::read_to_string(&file).into_diagnostic()?;
                let mut value: toml::Table = toml::from_str(&content).into_diagnostic()?;
                if let Some(explicit) = value.get("namespace") {
                    if explicit.as_str() != Some(&namespace) {
                        miette::bail!("external configuration namespace must match '{namespace}'");
                    }
                } else {
                    value.insert("namespace".into(), toml::Value::String(namespace.clone()));
                }
                PitchforkToml::parse_str(
                    &toml::to_string(&value).into_diagnostic()?,
                    &dir.join("pitchfork.toml"),
                )?;
                extra_configs::add(&namespace, &dir, &file)?;
                Ok(())
            }
            Some(Commands::Remove(args)) => {
                extra_configs::remove(&env::expand_tilde(&args.file))?;
                Ok(())
            }
            Some(Commands::List(args)) => args.run(),
            None => self.list.run(),
        }
    }
}

impl List {
    fn run(&self) -> Result<()> {
        let mut entries = extra_configs::entries();
        for entry in &mut entries {
            if entry.source == "env" {
                entry.namespace = PitchforkToml::namespace_for_project_dir(&entry.dir)?;
            }
        }
        if self.json {
            return crate::cli::json_output::print_json(&entries);
        }
        for entry in entries {
            for file in entry.config {
                println!(
                    "{}\t{}\t{}\t{}",
                    entry.namespace,
                    entry.dir.display(),
                    file.display(),
                    entry.source
                );
            }
        }
        Ok(())
    }
}
