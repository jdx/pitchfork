use crate::Result;
use crate::ipc::client::IpcClient;
use crate::{daemon_id::DaemonId, env, pitchfork_toml::PitchforkToml};
use miette::IntoDiagnostic;

fn resolve_daemon_filters<F>(ids: &[String], local_namespace: F) -> Result<Vec<DaemonId>>
where
    F: FnOnce() -> Result<String>,
{
    let local_namespace = ids
        .iter()
        .any(|id| !id.contains('/'))
        .then(local_namespace)
        .transpose()?;
    ids.iter()
        .map(|id| {
            if id.contains('/') {
                DaemonId::parse(id)
            } else {
                DaemonId::try_new(
                    local_namespace.as_deref().expect("bare ID needs namespace"),
                    id,
                )
            }
        })
        .collect()
}

/// Removes stopped/failed daemons from `pitchfork list`
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = "\
Removes stopped/failed daemons from `pitchfork list`

Cleans up the daemon list by removing entries for daemons that are no
longer running. Does not affect running daemons or their configurations.

Use this to clear out old entries after stopping daemons manually or
after daemons have failed.

Examples:

    pitchfork clean                 Remove all stopped/failed entries
    pitchfork clean my-worktree     Remove entries in one namespace
    pitchfork clean --daemon api    Remove the local namespace's api entry
    pitchfork clean --prune         Also forget projects whose directories disappeared

With --prune, entries are removed when their working directory no longer
exists: stopped daemon records, plus `[namespaces]` and `[slugs]` entries in
the global config and external config files (`pitchfork config add`) that
point at deleted paths. Namespaces with a running daemon are never touched.
    pitchfork c                     Alias for 'clean'"
)]
pub struct Clean {
    /// Only clean daemon registrations in these namespaces
    #[usage(name = "NAMESPACE")]
    namespaces: Vec<String>,
    /// Only clean these daemons (repeatable; bare names use the current namespace)
    #[usage(long = "daemon", value_name = "ID")]
    daemons: Vec<String>,
    /// Only clean registrations whose working directories no longer exist.
    /// Also prunes stale namespaces, slugs and config files from the global config
    #[usage(long)]
    prune: bool,
}

impl Clean {
    pub async fn run(&self) -> Result<()> {
        if self.namespaces.is_empty() && self.daemons.is_empty() && !self.prune {
            let ipc = IpcClient::connect(false).await?;
            ipc.clean().await?;
            return Ok(());
        }

        // Validate namespace arguments using the same rules as daemon IDs.
        for namespace in &self.namespaces {
            DaemonId::try_new(namespace, "probe")?;
        }
        let daemons = resolve_daemon_filters(&self.daemons, || {
            PitchforkToml::namespace_for_dir(&env::CWD)
        })?;
        let ipc = IpcClient::connect(false).await?;
        let scoped_to_daemons = !daemons.is_empty();
        let count = ipc
            .clean_filtered(self.namespaces.clone(), daemons, self.prune)
            .await?;
        info!("Cleaned up {count} stopped/failed daemon registration(s)");
        if self.prune && !scoped_to_daemons {
            // Never touch a namespace that still has a running daemon.
            let keep: std::collections::HashSet<String> = ipc
                .active_daemons()
                .await?
                .iter()
                .map(|d| d.id.namespace().to_string())
                .collect();
            let namespaces = self.namespaces.clone();
            let report = tokio::task::spawn_blocking(move || {
                PitchforkToml::prune_stale_registrations(&keep, &namespaces)
            })
            .await
            .into_diagnostic()??;
            if !report.is_empty() {
                info!(
                    "Pruned {} namespace(s), {} slug(s) and {} config file(s) whose paths no longer exist",
                    report.namespaces.len(),
                    report.slugs.len(),
                    report.config_files
                );
                if !report.slugs.is_empty()
                    && let Err(e) = ipc.sync_mdns().await
                {
                    warn!("could not refresh mDNS after pruning slugs: {e}");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_filters_do_not_resolve_local_namespace() {
        let ids = resolve_daemon_filters(&["other/api".to_string()], || {
            panic!("qualified daemon filters must not inspect local config")
        })
        .unwrap();
        assert_eq!(ids, vec![DaemonId::new("other", "api")]);
    }

    #[test]
    fn bare_filters_use_local_namespace() {
        let ids = resolve_daemon_filters(&["api".to_string()], || Ok("local".to_string())).unwrap();
        assert_eq!(ids, vec![DaemonId::new("local", "api")]);
    }
}
