# TUI Dashboard

Pitchfork includes a full-featured terminal UI for monitoring and managing daemons without leaving your terminal.

## Launch the TUI

```bash
pitchfork tui
```

The TUI connects to the supervisor automatically, starting it if needed.

## Features

### Dashboard View

- Live daemon status with color-coded states
- CPU and memory usage per daemon
- Fuzzy search to filter daemons
- Multi-select for batch operations
- Sortable columns

### Log Viewer

- Real-time log streaming
- Search within logs
- Follow mode (auto-scroll)
- Expandable full-screen view

### Vim-Style Navigation

The TUI uses familiar vim keybindings for efficient navigation.

## Keybindings

### Dashboard

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `l` / `Enter` | View logs |
| `i` | View details |
| `s` | Start daemon |
| `x` | Stop daemon (with confirmation) |
| `r` | Restart daemon |
| `e` | Enable daemon |
| `d` | Disable daemon (with confirmation) |
| `/` | Search/filter daemons |
| `Space` | Toggle selection |
| `Ctrl+a` | Select all visible |
| `c` | Clear selection |
| `a` | Toggle showing available daemons |
| `S` | Cycle sort column |
| `o` | Toggle sort order |
| `R` | Refresh |
| `?` | Show help |
| `q` / `Esc` | Quit |

### Log Viewer

| Key | Action |
|-----|--------|
| `j` / `↓` | Scroll down |
| `k` / `↑` | Scroll up |
| `Ctrl+d` | Page down |
| `Ctrl+u` | Page up |
| `g` | Jump to top |
| `G` | Jump to bottom |
| `f` | Toggle follow mode |
| `e` | Toggle expanded view |
| `/` | Search in logs |
| `n` | Next search match |
| `N` | Previous search match |
| `q` / `Esc` | Back to dashboard |

## Multi-Select Operations

Select multiple daemons with `Space`, then use `s`, `x`, `r`, `e`, or `d` to perform batch operations on all selected daemons.

## TUI vs Web UI

| Feature | TUI | Web UI |
|---------|-----|--------|
| Access | Terminal | Browser |
| Keybindings | Vim-style | Point and click |
| Log search | Built-in | Scroll only |
| Config editing | No | Yes |
| Best for | Terminal power users | Quick visual checks |

Both interfaces connect to the same supervisor, so changes in one are reflected in the other.
