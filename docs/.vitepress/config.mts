import { defineConfig } from "vitepress";

import spec from "../cli/commands.json";

interface Cmd {
  name: string;
  full_cmd: string[];
  subcommands: Record<string, Cmd>;
  hide?: boolean;
}

function getCommands(cmd: Cmd): string[][] {
  const commands: string[][] = [];
  for (const [name, sub] of Object.entries(cmd.subcommands)) {
    if (sub.hide) continue;
    commands.push(sub.full_cmd);
    commands.push(...getCommands(sub));
  }
  return commands;
}

const commands = getCommands(spec.cmd);

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "pitchfork",
  description: "A devilishly good process manager for developers",
  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: "Home", link: "/" },
      { text: "Quick Start", link: "/quickstart" },
      { text: "Guides", link: "/guides/shell-hook" },
      { text: "CLI Reference", link: "/cli" },
    ],

    sidebar: [
      {
        text: "Getting Started",
        items: [
          { text: "Quick Start", link: "/quickstart" },
          { text: "Installation", link: "/installation" },
          { text: "Your First Project", link: "/first-daemon" },
        ],
      },
      {
        text: "How-To Guides",
        items: [
          { text: "Shell Hook (Auto Start/Stop)", link: "/guides/shell-hook" },
          { text: "Ready Checks", link: "/guides/ready-checks" },
          { text: "File Watching", link: "/guides/file-watching" },
          { text: "Auto Restart on Failure", link: "/guides/auto-restart" },
          { text: "Cron Scheduling", link: "/guides/scheduling" },
          { text: "Start on Boot", link: "/guides/boot-start" },
          { text: "Log Management", link: "/guides/logs" },
          { text: "TUI Dashboard", link: "/guides/tui" },
          { text: "Web UI", link: "/guides/web-ui" },
          { text: "mise Integration", link: "/guides/mise-integration" },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "Configuration", link: "/reference/configuration" },
          { text: "Environment Variables", link: "/reference/environment-vars" },
          { text: "File Locations", link: "/reference/file-locations" },
          {
            text: "CLI Reference",
            link: "/cli",
            collapsed: true,
            items: commands.map((cmd) => ({
              text: cmd.join(" "),
              link: `/cli/${cmd.join("/")}`,
            })),
          },
        ],
      },
      {
        text: "Concepts",
        collapsed: true,
        items: [
          { text: "How Pitchfork Works", link: "/concepts/how-it-works" },
          { text: "Architecture", link: "/concepts/architecture" },
        ],
      },
      {
        text: "Resources",
        collapsed: true,
        items: [
          { text: "Troubleshooting", link: "/troubleshooting" },
        ],
      },
    ],

    socialLinks: [{ icon: "github", link: "https://github.com/jdx/pitchfork" }],

    logo: "/img/android-chrome-192x192.png",

    footer: {
      message: 'Released under the MIT License.',
      copyright: 'Forged in the fires below'
    },

    editLink: {
      pattern: 'https://github.com/jdx/pitchfork/edit/main/docs/:path',
      text: 'Edit this page on GitHub'
    }
  },
  head: [
    ["link", { rel: "icon", href: "/img/favicon.ico" }],
    ["meta", { name: "theme-color", content: "#dc2626" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:title", content: "pitchfork" }],
    ["meta", { property: "og:description", content: "A devilishly good process manager for developers" }],
  ],
  search: {
    provider: "local",
  },
  // Ignore localhost URLs in CLI examples
  ignoreDeadLinks: [
    /^http:\/\/localhost/,
    /^http:\/\/127\.0\.0\.1/,
  ]
});
