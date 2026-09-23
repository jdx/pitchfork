<template>
  <div class="pf-home">
    <section class="pf-hero" aria-labelledby="hero-title">
      <div class="pf-hero-copy">
        <h1 id="hero-title">A process manager for local development</h1>
        <p class="pf-intro">
          Pitchfork runs your development services in the background so you don’t
          need to keep a terminal open for each one. Define your API, database,
          and workers in <code>pitchfork.toml</code>, then start what you need from
          any terminal.
        </p>
        <div class="pf-actions">
          <a class="pf-button pf-button-primary" href="/quickstart"
            >Get started <span aria-hidden="true">→</span></a
          >
          <a class="pf-button pf-button-secondary" href="/first-daemon"
            >Build your first project</a
          >
        </div>
        <p class="pf-install">
          <span aria-hidden="true">$</span> <code>mise use -g pitchfork</code
          ><a href="/installation" aria-label="See all installation methods"
            >More ways to install <span aria-hidden="true">↗</span></a
          >
        </p>
      </div>
      <div class="pf-session" aria-label="Example development session">
        <div class="pf-window-bar">
          <span class="pf-window-dots" aria-hidden="true"
            ><i></i><i></i><i></i></span
          ><span>~/projects/my-app</span
          ><span class="pf-window-label">example session</span>
        </div>
        <div class="pf-terminal">
          <p class="pf-command"><span>$</span> pitchfork start api</p>
          <div class="pf-process">
            <span class="pf-check" aria-hidden="true">✓</span
            ><strong>redis</strong><span>ready</span><code>:6379</code>
          </div>
          <div class="pf-dependency">└─ dependency ready. Starting api.</div>
          <div class="pf-process">
            <span class="pf-check" aria-hidden="true">✓</span
            ><strong>api</strong><span>ready</span><code>:3000</code>
          </div>
          <div class="pf-terminal-rule"></div>
          <p class="pf-command"><span>$</span> pitchfork start api</p>
          <p class="pf-terminal-note">Already running.</p>
          <p class="pf-command pf-last-command">
            <span>$</span> <span class="pf-cursor" aria-hidden="true"></span>
          </p>
        </div>
        <div class="pf-session-footer">
          <span class="pf-status-dot" aria-hidden="true"></span> Services managed across projects
        </div>
        <img
          class="pf-mascot"
          src="/img/logo.png"
          width="116"
          height="116"
          alt=""
        />
      </div>
    </section>

    <div class="pf-principles" aria-label="At a glance">
      <span>Any language</span><span>One TOML file</span
      ><span>Project-aware</span><span>Open source · MIT</span>
    </div>

    <section class="pf-workflow" aria-labelledby="workflow-title">
      <div class="pf-section-copy">
        <h2 id="workflow-title">How it works</h2>
        <p>
          Use the same commands you already run. Pitchfork starts dependencies
          in order, waits for readiness checks, and collects logs. Starting a
          service that’s already running is safe, so you can switch terminals or
          return to a project without remembering what you left running.
        </p>
        <a class="pf-text-link" href="/first-daemon"
          >Walk through this setup <span aria-hidden="true">→</span></a
        >
      </div>
      <div class="pf-config-panel">
        <div class="pf-window-bar">
          <span class="pf-file-icon" aria-hidden="true">≡</span
          ><span>pitchfork.toml</span
          ><span class="pf-window-label">project config</span>
        </div>
        <pre v-pre><code><span class="pf-code-section">[daemons.redis]</span>
run = <span class="pf-code-string">"redis-server --port $PORT"</span>
port = <span class="pf-code-number">6379</span>
ready_cmd = <span class="pf-code-string">"redis-cli -p $PORT ping"</span>

<span class="pf-code-section">[daemons.api]</span>
run = <span class="pf-code-string">"node server.js"</span>
port = <span class="pf-code-number">3000</span>
depends = [<span class="pf-code-string">"redis"</span>]
ready_http = <span class="pf-code-string">"http://localhost:3000/health"</span>
retry = <span class="pf-code-number">3</span></code></pre>
        <p class="pf-config-caption">
          The API starts after Redis passes its readiness check.
        </p>
      </div>
    </section>

    <section class="pf-features" aria-labelledby="features-title">
      <div class="pf-section-heading">
        <h2 id="features-title">Automate what you need</h2>
        <p>
          You can configure services to
          <a href="/guides/file-watching">restart when files change</a>,
          <a href="/guides/auto-restart">retry after failures</a>, or
          <a href="/guides/shell-hook"
            >start and stop as you move between projects</a
          >.
          <a href="/guides/port-management">Port management and local URLs</a>
          help you run multiple projects and worktrees without manually keeping
          track of addresses.
        </p>
      </div>
    </section>

    <section class="pf-dashboard" aria-labelledby="dashboard-title">
      <div class="pf-section-heading">
        <h2 id="dashboard-title">See what’s running</h2>
        <p>
          Check status and follow logs from the CLI, or use the terminal or web
          dashboard to manage services across projects.
        </p>
        <div class="pf-dashboard-links">
          <a class="pf-text-link" href="/guides/tui"
            >Terminal dashboard <span aria-hidden="true">→</span></a
          ><a class="pf-text-link" href="/guides/web-ui"
            >Web dashboard <span aria-hidden="true">→</span></a
          >
        </div>
      </div>
      <a
        class="pf-dashboard-image"
        href="/guides/web-ui"
        aria-label="Learn how to enable the web dashboard"
        ><img
          src="/img/webui-pc.png"
          alt="Pitchfork web dashboard showing daemon status and controls"
          width="2090"
          height="1850"
          loading="lazy"
          decoding="async"
      /></a>
    </section>
  </div>
</template>
