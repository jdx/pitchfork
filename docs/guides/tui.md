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

### Config Editor

- Create new daemons with a form-based interface
- Edit existing daemon configurations
- Delete daemons from config files
- Validation for required fields and formats

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
| `n` | Create new daemon |
| `E` | Edit selected daemon config |
| `?` | Show help |
| `q` / `Esc` | Quit |

### Config Editor

| Key | Action |
|-----|--------|
| `Tab` / `j` / `↓` | Next field |
| `Shift+Tab` / `k` / `↑` | Previous field |
| `Enter` | Edit text field |
| `Space` | Toggle checkbox / cycle option |
| `Ctrl+s` | Save configuration |
| `D` | Delete daemon (edit mode only) |
| `q` / `Esc` | Cancel (confirms if unsaved changes) |

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
