# MCP Server (AI Assistants)

Pitchfork includes a built-in [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) server, allowing AI assistants to manage your daemons directly.

## What is MCP?

MCP is an open protocol that lets AI assistants interact with external tools via JSON-RPC over stdin/stdout. Pitchfork's MCP server exposes daemon management operations so AI coding assistants can start, stop, restart, and monitor your development services.

## Setup

### Claude Desktop

Add to your `claude_desktop_config.json`:

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

### Cursor

Add to your Cursor MCP configuration:

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

### Other MCP-Compatible Tools

Any tool that supports the MCP protocol can use pitchfork. The server communicates over stdin/stdout using JSON-RPC.

## Available Tools

The MCP server exposes five tools:

| Tool | Description |
|------|-------------|
| `pitchfork_status` | List all daemons and their current state (PID, status, errors) |
| `pitchfork_start` | Start one or more daemons by name (supports `force` for restart) |
| `pitchfork_stop` | Stop one or more daemons by name |
| `pitchfork_restart` | Restart one or more daemons (equivalent to start with force) |
| `pitchfork_logs` | Return recent log output for one or more daemons (default: 50 lines) |

## Usage Examples

Once configured, you can ask your AI assistant things like:

- "What daemons are running?"
- "Start the API server"
- "Restart the worker daemon"
- "Show me the logs for the API"
- "Stop all daemons"

The AI assistant will use the MCP tools to execute these operations through pitchfork.


