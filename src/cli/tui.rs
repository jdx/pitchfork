use crate::Result;
use crate::daemon_list::NamespaceFilter;

/// Launch the interactive TUI dashboard
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = "\
Launch the interactive TUI dashboard

Shows live daemon status with fuzzy search, sorting, batch operations,
log viewing, and a config editor.

The dashboard can be scoped to one or more namespaces; the fuzzy search
(`/`) then operates within the scoped set.

Example:

    pitchfork tui
    pitchfork tui --namespace frontend
                                    Show only daemons in the 'frontend' namespace
    pitchfork tui --project         Show only the current project's daemons"
)]
pub struct Tui {
    /// Only show daemons in this namespace (repeatable for OR logic)
    #[usage(long)]
    namespace: Vec<String>,

    /// Only show daemons in the current project's namespace
    ///
    /// The namespace is resolved from the current directory the same way
    /// short daemon IDs are: the nearest config file's namespace, falling
    /// back to 'global' when no config file is found.
    #[usage(long)]
    project: bool,
}

impl Tui {
    pub async fn run(&self) -> Result<()> {
        let filter = NamespaceFilter::from_flags(&self.namespace, self.project)?;
        crate::tui::run(filter).await
    }
}
