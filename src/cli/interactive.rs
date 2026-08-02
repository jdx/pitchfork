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

/// If stdout is a TTY, prompt the user to select one or more daemons from
/// `candidates`. Returns the selected `DaemonId`s.
///
/// If stdout is not a TTY (piped/redirected), returns an error instructing the
/// user to provide daemon IDs explicitly, preserving the original non-interactive
/// behavior.
pub(crate) fn select_daemons_interactively(
    candidates: &[DaemonId],
    action: &str,
) -> Result<Vec<DaemonId>> {
    if candidates.is_empty() {
        miette::bail!("No daemons available to {action}");
    }

    if !std::io::stdout().is_terminal() {
        miette::bail!(
            "No daemon ID specified. Provide one or more daemon IDs, \
             or run in an interactive terminal to use the selection prompt."
        );
    }

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
