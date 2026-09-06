<script setup lang="ts">
const features = [
  {
    number: "01",
    title: "Start once. Stay ready.",
    text: "Run a command again without launching a duplicate. Wait for dependencies to accept requests before starting the next service.",
    link: "/guides/ready-checks",
    action: "Define readiness",
  },
  {
    number: "02",
    title: "Let services follow you.",
    text: "Enter a project and start its services. Leave the last session and let them stop. Keep separate names for separate worktrees.",
    link: "/guides/shell-hook",
    action: "Connect your shell",
  },
  {
    number: "03",
    title: "Keep the feedback loop short.",
    text: "Restart after a source edit, retry a crashed worker, or catch a service that is running but no longer responding.",
    link: "/guides/health-checks",
    action: "Keep services healthy",
  },
  {
    number: "04",
    title: "Give every service a home.",
    text: "Assign ports, handle conflicts, and use a stable local URL even when the underlying port changes.",
    link: "/guides/port-management",
    action: "Set up local URLs",
  },
  {
    number: "05",
    title: "See what is happening.",
    text: "Follow logs across services. Filter structured output by level or field. Inspect processes in a terminal or browser dashboard.",
    link: "/guides/logs",
    action: "Find the right logs",
  },
  {
    number: "06",
    title: "Use the tools you have.",
    text: "Run shell commands in any language, activate your mise environment, or connect an assistant through the built-in MCP server.",
    link: "/guides/",
    action: "Explore integrations",
  },
];
</script>

<template>
  <div class="pf-home">
    <section class="pf-hero" aria-labelledby="hero-title">
      <div class="pf-hero-copy">
        <p class="pf-eyebrow">
          <span class="pf-status-dot" aria-hidden="true"></span> Development,
          with fewer loose ends
        </p>
        <h1 id="hero-title">
          Your project.<br /><span>Already running.</span>
        </h1>
        <p class="pf-intro">
          Your API, database, and workers, started with one command. Pitchfork
          handles readiness, retries, and logs so you can get back to work.
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
          <p class="pf-terminal-note">Already running. You're good to go.</p>
          <p class="pf-command pf-last-command">
            <span>$</span> <span class="pf-cursor" aria-hidden="true"></span>
          </p>
        </div>
        <div class="pf-session-footer">
          <span class="pf-status-dot" aria-hidden="true"></span> One supervisor.
          Every project.
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
        <p class="pf-eyebrow">A little config. A lot less juggling.</p>
        <h2 id="workflow-title">
          Check in the setup.<br />Check off the busywork.
        </h2>
        <p>
          Put the commands you already run in <code>pitchfork.toml</code>. Give
          each service a name, declare what it needs, and share the setup with
          your team.
        </p>
        <ol class="pf-steps">
          <li>
            <span>1</span>
            <div>
              <strong>Describe your services</strong>
              <p>Commands, environment, and dependencies in one place.</p>
            </div>
          </li>
          <li>
            <span>2</span>
            <div>
              <strong>Start what you need</strong>
              <p>Dependencies start first. Running services stay running.</p>
            </div>
          </li>
          <li>
            <span>3</span>
            <div>
              <strong>Work from any terminal</strong>
              <p>Inspect, restart, or stop services whenever you need to.</p>
            </div>
          </li>
        </ol>
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
          Bring your own commands. Pitchfork takes care of their lifecycle.
        </p>
      </div>
    </section>

    <section class="pf-features" aria-labelledby="features-title">
      <div class="pf-section-heading">
        <p class="pf-eyebrow">Made for the way you develop</p>
        <h2 id="features-title">
          Less process management.<br />More making things.
        </h2>
      </div>
      <div class="pf-feature-grid">
        <a
          v-for="feature in features"
          :key="feature.number"
          class="pf-feature"
          :href="feature.link"
        >
          <span class="pf-feature-number" aria-hidden="true">{{
            feature.number
          }}</span>
          <h3>{{ feature.title }}</h3>
          <p>{{ feature.text }}</p>
          <span class="pf-text-link"
            >{{ feature.action }} <span aria-hidden="true">→</span></span
          >
        </a>
      </div>
    </section>

    <section class="pf-dashboard" aria-labelledby="dashboard-title">
      <div class="pf-section-heading">
        <p class="pf-eyebrow">From the big picture to the last log line</p>
        <h2 id="dashboard-title">A place for every process.</h2>
        <p>
          See what is running, find what failed, and open the logs.<br
            class="pf-desktop-break"
          />
          Use <code>pitchfork tui</code> in your terminal or enable the web
          dashboard.
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

    <section class="pf-next" aria-labelledby="next-title">
      <div>
        <p class="pf-eyebrow">Summon your daemons.</p>
        <h2 id="next-title">Then get on with your day.</h2>
      </div>
      <div class="pf-actions">
        <a class="pf-button pf-button-primary" href="/quickstart"
          >Run your first daemon <span aria-hidden="true">→</span></a
        ><a class="pf-text-link" href="/cli/"
          >Browse the CLI <span aria-hidden="true">↗</span></a
        >
      </div>
    </section>
  </div>
</template>
