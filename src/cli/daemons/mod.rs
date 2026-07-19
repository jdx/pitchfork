mod add;
mod remove;

use crate::cli::json_output::{JsonDaemonConfigEntry, print_json};
use crate::pitchfork_toml::PitchforkToml;
use crate::{Result, env};
use miette::{IntoDiagnostic, bail};
use std::path::{Path, PathBuf};

pub use add::Add;
pub use remove::Remove;

fn is_project_config_path(path: &Path) -> bool {
    path.file_name()
        .map(|name| name == "pitchfork.toml" || name == "pitchfork.local.toml")
        .unwrap_or(false)
}

/// Resolve the path to a project-level config file based on `--local`/`--project` flags.
///
/// This is the shared logic used by `pf daemons add`, `pf daemons remove`, and
/// `pf settings set` to determine which project-level config file to write to.
///
/// Resolution rules:
/// - `--local`: Find or create `pitchfork.local.toml` next to the nearest `pitchfork.toml`.
///   If no `pitchfork.toml` exists, fall back to `pitchfork.local.toml` in CWD.
/// - `--project`: Find or create `pitchfork.toml` in the project hierarchy.
///   If none exists, fall back to `pitchfork.toml` in CWD.
/// - Default (no flag): Same as `--project`.
///
/// The `exists_filter` parameter controls whether only existing files are considered.
/// `add` needs to be able to create new files, so it passes `false`.
/// `remove` only operates on existing files, so it passes `true`.
pub(crate) async fn resolve_project_config_path(
    local: bool,
    project: bool,
    exists_filter: bool,
) -> Result<PathBuf> {
    if local && project {
        bail!("cannot specify both --local and --project");
    }

    let paths = PitchforkToml::list_paths();
    let mut project_paths = Vec::new();
    for p in &paths {
        if !is_project_config_path(p) {
            continue;
        }
        if exists_filter {
            if tokio::fs::try_exists(p).await.unwrap_or(false) {
                project_paths.push(p.clone());
            }
        } else {
            project_paths.push(p.clone());
        }
    }

    if local {
        if let Some(p) = project_paths
            .iter()
            .find(|p| {
                p.file_name()
                    .map(|n| n == "pitchfork.local.toml")
                    .unwrap_or(false)
            })
            .cloned()
        {
            return Ok(p);
        }
        let dir = project_paths
            .iter()
            .find(|p| {
                p.file_name()
                    .map(|n| n == "pitchfork.toml")
                    .unwrap_or(false)
            })
            .and_then(|p| p.parent())
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| env::CWD.clone());
        return Ok(dir.join("pitchfork.local.toml"));
    }

    if project {
        if let Some(p) = project_paths
            .iter()
            .find(|p| {
                p.file_name()
                    .map(|n| n == "pitchfork.toml")
                    .unwrap_or(false)
            })
            .cloned()
        {
            return Ok(p);
        }
        return Ok(env::CWD.join("pitchfork.toml"));
    }

    Ok(project_paths
        .iter()
        .find(|p| {
            p.file_name()
                .map(|n| n == "pitchfork.toml")
                .unwrap_or(false)
        })
        .cloned()
        .unwrap_or_else(|| env::CWD.join("pitchfork.toml")))
}

/// Resolve the config file path based on `--global`/`--local`/`--project` flags.
///
/// This is the shared logic used by `pf daemons add`, `pf daemons remove`, and
/// `pf settings set` to determine which config file to write to.
///
/// Resolution rules:
/// - `--global`: Write to the user-level global config
///   (`~/.config/pitchfork/config.toml`). Mutually exclusive with `--local`/`--project`.
/// - `--local`/`--project`/default: Delegates to [`resolve_project_config_path`].
///
/// The `exists_filter` parameter is forwarded to [`resolve_project_config_path`]
/// and only affects the project-level branches. `add` and `settings set` pass
/// `false` (they may create new files); `remove` passes `true` (it only
/// operates on existing files).
pub(crate) async fn resolve_config_path(
    global: bool,
    local: bool,
    project: bool,
    exists_filter: bool,
) -> Result<PathBuf> {
    if global && (local || project) {
        bail!("cannot combine --global with --local or --project");
    }
    if global {
        return Ok(env::PITCHFORK_GLOBAL_CONFIG_USER.clone());
    }
    resolve_project_config_path(local, project, exists_filter).await
}

/// List configured daemons from all merged config files.
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "daemon",
    verbatim_doc_comment,
    args_conflicts_with_subcommands = true
)]
pub struct Daemons {
    #[clap(subcommand)]
    command: Option<DaemonsCommand>,

    /// Output in JSON format
    #[clap(long)]
    json: bool,
}

#[derive(Debug, clap::Subcommand)]
enum DaemonsCommand {
    Add(Box<Add>),
    Remove(Remove),
}

impl Daemons {
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            Some(DaemonsCommand::Add(add)) => add.run().await,
            Some(DaemonsCommand::Remove(remove)) => remove.run().await,
            None => {
                let config = tokio::task::spawn_blocking(PitchforkToml::all_merged)
                    .await
                    .into_diagnostic()??;
                if self.json {
                    let entries: Vec<JsonDaemonConfigEntry> = config
                        .daemons
                        .iter()
                        .map(|(id, daemon)| JsonDaemonConfigEntry {
                            id: id.qualified(),
                            run: daemon.run.clone(),
                        })
                        .collect();
                    print_json(&entries)
                } else if config.daemons.is_empty() {
                    println!("No daemons configured.");
                    Ok(())
                } else {
                    for (id, daemon) in &config.daemons {
                        println!("{id}\t{}", daemon.run);
                    }
                    Ok(())
                }
            }
        }
    }
}
