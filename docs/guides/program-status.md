# Reporting Status from a Daemon

A daemon can tell Pitchfork what it is doing right now, such as a phase or a progress percentage, without anyone reading its logs. Pitchfork understands the [Program Status Protocol (OSC 7501)](https://www.superlogical.com/rex/docs/build/program-status), a small terminal escape sequence. The latest report appears in `pitchfork list`, `pitchfork status`, the TUI and the web UI.

```bash
# in a script run by a daemon
report() { printf '\033]7501;state=%s:progress=%s:msg=%s\033\\' "$1" "$2" "$(printf %s "$3" | base64)"; }
report working 40 "Syncing GitHub issues"
```

```
$ pitchfork list
Name         Status   
nightly-sync running  working 40% Syncing GitHub issues
```

## Format

`ESC ] 7501 ; key=value : key=value ST`, where `ST` is `ESC \` or BEL.

| Key | Value |
|-----|-------|
| `state` | Required: `idle`, `working`, `done`, `blocked`, `error`, or `clear` to remove the report |
| `progress` | 0 to 100, kept only for `working` and `blocked` |
| `msg` | One line of text, base64-encoded UTF-8 (up to 2048 bytes decoded) |

A report whose `msg` contains control characters, or breaks a size limit, is ignored. Reports with an `id` describe child records and are ignored; only the root record is tracked.

## Lifecycle

- `idle`, `working` and `blocked` reports are dropped when the process exits.
- `done` and `error` reports are kept after exit until the next run starts.
- Starting a daemon again begins with no report.
- Reports are read from the daemon's output, stdout and stderr, or the terminal when `pty = true`. The line stays in the logs as usual.
- `pitchfork list --json` and `pitchfork status --json` include a `program_status` object when a report exists.
