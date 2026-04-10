---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: "pitchfork"
  text: "Summon Your Daemons"
  tagline: A devilishly good process manager for developers
  image:
    src: /img/logo.png
    alt: pitchfork logo
  actions:
    - theme: brand
      text: Quick Start
      link: /quickstart
    - theme: alt
      text: CLI Reference
      link: /cli

features:
  - icon: "\U0001F525"
    title: Raise the Dead
    details: Start daemons only if they're not already running. No more duplicate processes haunting your system.
    link: /first-daemon
  - icon: "\U0001F608"
    title: Eternal Vigilance
    details: Automatic restarts on failure with configurable retry limits and exponential backoff. Your daemons will rise again.
    link: /guides/auto-restart
  - icon: "\u26A1"
    title: Ready When Summoned
    details: Smart ready checks via delay, output patterns, HTTP endpoints, TCP ports, or custom commands. Know when your daemon is truly alive.
    link: /guides/ready-checks
  - icon: "\U0001F3E0"
    title: Shell Possession
    details: Auto-start daemons when entering a project directory, auto-stop when leaving. Seamless possession.
    link: /guides/shell-hook
  - icon: "\U0001F517"
    title: Dependency Chains
    details: Declare daemon dependencies for automatic topological start ordering and parallel execution within each level.
    link: /reference/configuration#depends
  - icon: "\U0001F4C2"
    title: File Watching
    details: Auto-restart daemons when source files change. Glob patterns with debouncing keep your dev loop tight.
    link: /guides/file-watching
  - icon: "\u23F0"
    title: Unholy Schedules
    details: Cron-style scheduling for periodic tasks with configurable retrigger modes (finish, always, success, fail).
    link: /guides/scheduling
  - icon: "\U0001F52E"
    title: Scrying Portals
    details: Peer into the abyss with a terminal TUI or web dashboard. Vim keybindings for the devoted, browser for the casual summoner.
    link: /guides/tui
  - icon: "\U0001F916"
    title: AI Integration
    details: Built-in MCP server lets AI assistants (Claude, Cursor) manage your daemons — start, stop, restart, and read logs.
    link: /guides/mcp
  - icon: "\U0001F6E1\uFE0F"
    title: Resource Limits
    details: Enforce memory and CPU limits per daemon. Transient spike protection with consecutive-sample thresholds.
    link: /reference/configuration#memory_limit
  - icon: "\U0001F4E6"
    title: Container Ready
    details: Run as PID 1 inside Docker with zombie reaping and graceful signal forwarding. Production-grade container entrypoint.
    link: /guides/container-mode
  - icon: "\U0001F3F7\uFE0F"
    title: Lifecycle Hooks
    details: Run commands on ready, fail, retry, stop, and exit events. Fire-and-forget with environment variable injection.
    link: /guides/lifecycle-hooks
---
