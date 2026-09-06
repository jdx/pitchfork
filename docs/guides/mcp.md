---
description: Connect an MCP-compatible assistant to pitchfork over stdio to inspect and manage development services.
---
# MCP server

Pitchfork includes a [Model Context Protocol](https://modelcontextprotocol.io/)
server. An MCP-compatible client can launch it as a subprocess to inspect,
start, stop, and restart daemons and read their logs.

## Connect a client

Configure a **stdio** server with command `pitchfork` and argument `mcp`.
For clients that use the `mcpServers` JSON format:

```json
{
  "mcpServers": {
    "pitchfork": {
      "command": "pitchfork",
      "args": ["mcp"]
    }
  }
}
```

Use your client's MCP settings to choose the configuration file and scope.
The client must be able to find `pitchfork` on `PATH`; if it cannot, replace
`command` with the absolute path to the installed binary.

For project-local configuration and short daemon names, launch the server in
the project directory if your client supports setting a working directory.
Otherwise use [qualified daemon IDs](/concepts/namespaces), such as `my-app/api`,
for known projects.

## Available tools

| Tool | What it does |
| --- | --- |
| `pitchfork_status` | List daemons and their current state |
| `pitchfork_start` | Start named daemons; supports forcing a restart |
| `pitchfork_stop` | Stop named daemons |
| `pitchfork_restart` | Restart named daemons |
| `pitchfork_logs` | Read recent output; defaults to 50 lines |

The MCP server uses the same supervisor as the CLI. Changes made through an
assistant are visible in `pitchfork list`, the TUI, and the web UI.

## Try it

Ask your assistant to list the daemons, inspect `my-app/api`, or show its recent
logs. Once the connection works, you can ask it to start or restart that service.
The client's approval settings determine when it asks before executing a tool.

If a daemon is missing, check the server's working directory and confirm it is
visible with `pitchfork list` from that directory. See
[the CLI reference](/cli/mcp) for the command's full help.
