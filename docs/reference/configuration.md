# Configuration Reference

Complete reference for `pitchfork.toml` configuration files.

## Configuration Hierarchy

Pitchfork loads configuration files in order, with later files overriding earlier ones:

1. **System-level:** `/etc/pitchfork/config.toml`
2. **User-level:** `~/.config/pitchfork/config.toml`
3. **Project-level:** `pitchfork.toml`/`pitchfork.local.toml` files from filesystem root to current directory (`pitchfork.local.toml` overrides `pitchfork.toml` in the same directory)

## JSON Schema

A JSON Schema is available for editor autocompletion and validation:

**URL:** [`https://pitchfork.dev/schema.json`](/schema.json)

### Editor Setup

**VS Code** with [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml):

```toml
#:schema https://pitchfork.dev/schema.json

[daemons.api]
run = "npm run server"
```

**JetBrains IDEs**: Add the schema URL in Settings → Languages & Frameworks → Schemas and DTDs → JSON Schema Mappings.

## File Format

All configuration uses TOML format:

```toml
[daemons.<daemon-name>]
run = "command to execute"
# ... other options
```

### Daemon Naming Rules

Daemon names must follow these rules:

| Rule | Valid | Invalid |
|------|-------|---------|
| No double dashes | `my-app` | `my--app` |
| No slashes | `api` | `api/v2` |
| No spaces | `my_app` | `my app` |
| No parent references | `myapp` | `..` or `foo..bar` |
| ASCII only | `myapp123` | `myäpp` |

The `--` sequence is reserved for internal use (namespace encoding). See [Namespaces](/concepts/namespaces) for details.

<script setup>
import ConfigTable from '../components/ConfigTable.vue'
</script>

## Daemon Options

<ConfigTable />