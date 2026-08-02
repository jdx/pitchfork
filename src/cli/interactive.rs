//! Interactive daemon selection via multi-select prompt.
//!
//! When the user runs `pf start` / `pf stop` / `pf restart` without specifying
//! any daemon, and stdin/stdout is a TTY, we launch a fuzzy-filterable
//! multi-select prompt ([`demand::MultiSelect`]) so the user can pick daemons
//! interactively.

use crate::Result;
use crate::daemon_id::DaemonId;
use demand::{DemandOption, MultiSelect};
use std::io::IsTerminal;

/// Returns `Ok(())` if both stdin and stdout are TTYs, so the interactive
/// prompt can read keyboard input and render the UI.
///
/// Call this before connecting to the supervisor (which may auto-start it)
/// to avoid side effects when the command is non-interactive.
pub(crate) fn require_interactive_terminal() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        miette::bail!(
            "No daemon ID specified. Provide one or more daemon IDs, \
             or run in an interactive terminal to use the selection prompt."
        );
    }
    Ok(())
}

/// Prompt the user to select one or more daemons from `candidates` via a
/// fuzzy-filterable multi-select prompt. Returns the selected `DaemonId`s.
///
/// Caller must ensure [`require_interactive_terminal`] has been checked
/// beforehand if the IPC connection has side effects (e.g. supervisor
/// auto-start).
pub(crate) fn select_daemons_interactively(
    candidates: &[DaemonId],
    action: &str,
) -> Result<Vec<DaemonId>> {
    if candidates.is_empty() {
        miette::bail!("No daemons available to {action}");
    }

    // Defense-in-depth: caller should have checked already, but verify again
    // in case this function is called from a context that didn't.
    require_interactive_terminal()?;

    let title = format!("Select daemon(s) to {action}");

    let mut sorted: Vec<DaemonId> = candidates.to_vec();
    sorted.sort();

    let mut ms = MultiSelect::new(&title)
        .filterable(true)
        .description("Use / to filter, space to toggle, enter to confirm");

    for id in &sorted {
        ms = ms.option(DemandOption::with_label(id.qualified(), id.clone()));
    }

    let selected: Vec<DaemonId> = ms.run().map_err(|e| {
        if e.kind() == std::io::ErrorKind::Interrupted {
            miette::miette!("Selection cancelled")
        } else {
            miette::miette!("Interactive selection failed: {e}")
        }
    })?;

    if selected.is_empty() {
        miette::bail!("No daemons selected");
    }

    Ok(selected)
}
