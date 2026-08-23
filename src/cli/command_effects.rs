//! What each pitchfork command does to the world.
//!
//! pitchfork's usage spec is derived from static usage-rs metadata. Most command
//! effects are centrally audited here and applied as sparse metadata overlays in
//! [`crate::cli::usage`].
//!
//! The three values are defined by the usage spec:
//!
//! - `read` — only inspects state; running it twice is the same as running it
//!   once, and not running it changes nothing.
//! - `write` — creates or modifies state, but removes nothing the user cannot
//!   recreate.
//! - `destructive` — removes something the user installed or configured, where
//!   getting it back means redoing work. Deserves a confirmation prompt.
//!
//! **An unlisted command means "unknown", not "safe".** Consumers treat the
//! absence of a value as "ask", so leaving a command out is the conservative
//! choice and mislabeling one `read` is the dangerous one.

use usage_rs::spec::Effect::{self as SpecCommandEffect, Destructive, Read, Write};

/// Commands whose effect is fixed, keyed by their full path under `pitchfork`.
pub const EFFECTS: &[(&str, SpecCommandEffect)] = &[
    ("activate", Read),
    ("api-schema", Read),
    ("boot", Read),
    ("boot disable", Write),
    ("boot enable", Write),
    ("boot status", Read),
    ("cd", Read),
    // Drops list entries for daemons that already stopped or failed; no
    // configuration or output is lost.
    ("clean", Write),
    ("completion", Read),
    ("daemons", Read),
    ("daemons add", Write),
    // Deletes a daemon the user wrote into a pitchfork config file.
    ("daemons remove", Destructive),
    ("disable", Write),
    ("enable", Write),
    ("list", Read),
    ("log-sink", Write),
    // `logs` reads; `--clear` deletes the stored logs. See FLAG_EFFECTS.
    ("logs", Read),
    ("project", Read),
    ("project enter", Write),
    ("project leave", Write),
    ("project list", Read),
    ("proxy", Read),
    ("proxy add", Write),
    // Deletes a slug mapping the user wrote into the global config.
    ("proxy remove", Destructive),
    ("proxy status", Read),
    // Both directions modify the system trust store, which is why neither is
    // `read`; each is undone by the other, so neither destroys anything.
    ("proxy trust", Write),
    ("proxy untrust", Write),
    ("schema", Read),
    ("settings", Read),
    ("settings get", Read),
    ("settings list", Read),
    ("settings set", Write),
    ("sponsors", Read),
    ("status", Read),
    ("stop", Write),
    ("supervisor", Read),
    ("supervisor status", Read),
    ("supervisor stop", Write),
    ("usage", Read),
    ("wait", Read),
];

/// Commands with no fixed effect, and why.
///
/// pitchfork exists to run daemons, and a daemon is whatever command the user
/// put in `pitchfork.toml`. For everything below, the effect is that command's
/// effect. Labelling them would be a guess about someone else's program.
// Only the coverage test reads this; it exists so the reason a command is
// left unclassified lives next to the decision rather than in a commit message.
#[cfg(test)]
pub const UNCLASSIFIED: &[(&str, &str)] = &[
    ("mcp", "serves tools that start and stop daemons on request"),
    ("restart", "stops then reruns a user-configured daemon"),
    ("run", "runs a one-off command as a daemon"),
    ("start", "runs a user-configured daemon"),
    ("supervisor run", "supervises user-configured daemons"),
    ("supervisor start", "supervises user-configured daemons"),
    (
        "tui",
        "an interactive dashboard that can start and stop daemons",
    ),
];

/// Flags that raise the effect of their command, keyed by (command, flag).
///
/// usage takes the effect of an invocation to be the maximum of the
/// command's effect and that of every flag supplied, so these only ever raise.
/// Most flags belong nowhere near this table — it is for the few that change
/// what the command does to the world.
#[cfg(test)]
pub const FLAG_EFFECTS: &[(&str, &str, SpecCommandEffect)] = &[
    // `LOG_STORE.clear()` — deletes the stored logs for the matched daemons.
    ("logs", "clear", Destructive),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use std::collections::HashSet;

    /// Every command in the tree, hidden ones included: a hidden command is
    /// still runnable.
    fn all_commands() -> Vec<String> {
        let mut out = vec![];
        collect(Cli::spec().root, &mut vec![], &mut out);
        out
    }

    fn collect(
        cmd: &usage_rs::spec::CommandMeta<'_>,
        path: &mut Vec<String>,
        out: &mut Vec<String>,
    ) {
        for sub in cmd.subcommands {
            path.push(sub.cmd.name.to_string());
            out.push(path.join(" "));
            collect(sub, path, out);
            path.pop();
        }
    }

    fn find_command(path: &str) -> &'static usage_rs::spec::CommandMeta<'static> {
        path.split_ascii_whitespace()
            .fold(Cli::spec().root, |cmd, name| {
                cmd.subcommands
                    .iter()
                    .copied()
                    .find(|sub| sub.cmd.name == name)
                    .unwrap_or_else(|| panic!("no `pitchfork {path}`"))
            })
    }

    fn classified() -> HashSet<&'static str> {
        EFFECTS
            .iter()
            .map(|(name, _)| *name)
            .chain(UNCLASSIFIED.iter().map(|(name, _)| *name))
            .collect()
    }

    /// The tables are only worth having if they reach the emitted spec.
    #[test]
    fn overlays_annotate_commands_and_flags() {
        let overlays: Vec<_> = EFFECTS
            .iter()
            .map(|(path, effect)| usage_rs::spec::CommandOverlay::effect(path, *effect))
            .collect();
        let kdl = Cli::spec().view().overlay(&overlays).to_kdl();
        let line = |command: &str| {
            kdl.lines()
                .find(|line| line.trim_start().starts_with(&format!("cmd {command} ")))
                .unwrap_or_else(|| panic!("no `pitchfork {command}` in:\n{kdl}"))
        };
        assert!(line("logs").contains("effect=read"), "{kdl}");
        assert!(line("stop").contains("effect=write"), "{kdl}");
        assert!(kdl.contains("effect=destructive"), "{kdl}");
    }

    /// Adding a command without deciding what it does to the world is the
    /// failure mode this table exists to prevent, so make it a test failure
    /// rather than a silently missing annotation.
    #[test]
    fn every_command_is_classified() {
        let known = classified();
        let missing: Vec<String> = all_commands()
            .into_iter()
            .filter(|cmd| !known.contains(cmd.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "these commands have no entry in EFFECTS or UNCLASSIFIED \
             (src/cli/command_effects.rs) — decide whether each is read, write, \
             destructive, or genuinely unclassifiable:\n  {}",
            missing.join("\n  ")
        );
    }

    /// Catches entries left behind by a renamed or removed command.
    #[test]
    fn no_classification_refers_to_a_missing_command() {
        let present: HashSet<String> = all_commands().into_iter().collect();
        let stale: Vec<&str> = classified()
            .into_iter()
            .filter(|name| !present.contains(*name))
            .collect();
        assert!(
            stale.is_empty(),
            "these entries no longer match a command:\n  {}",
            stale.join("\n  ")
        );
    }

    /// A renamed or removed flag would otherwise silently stop being annotated.
    #[test]
    fn every_flag_effect_matches_a_real_flag() {
        let mut missing = vec![];
        for (cmd_path, flag_name, _) in FLAG_EFFECTS {
            if !find_command(cmd_path)
                .flags
                .iter()
                .any(|flag| flag.flag.name == *flag_name)
            {
                missing.push(format!("{cmd_path} --{flag_name}"));
            }
        }
        assert!(
            missing.is_empty(),
            "these FLAG_EFFECTS entries do not match a real flag:\n  {}",
            missing.join("\n  ")
        );
    }

    #[test]
    fn classifications_are_not_duplicated() {
        let mut seen = HashSet::new();
        for name in EFFECTS
            .iter()
            .map(|(n, _)| *n)
            .chain(UNCLASSIFIED.iter().map(|(n, _)| *n))
        {
            assert!(seen.insert(name), "{name} is classified twice");
        }
    }
}
