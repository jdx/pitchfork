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
      { text: "Getting Started", link: "/getting-started" },
      { text: "CLI Reference", link: "/cli" },
    ],

    sidebar: [
      { text: "Getting Started", link: "/getting-started" },
      { text: "Integration with mise", link: "/mise" },
      { text: "Ready Checks", link: "/ready-checks" },
      { text: "Retry on Failure", link: "/retry" },
      { text: "Cron Scheduling", link: "/cron" },
      { text: "Start on Boot", link: "/boot-start" },
      { text: "Configuration", link: "/config" },
      { text: "Architecture", link: "/architecture" },
      {
        text: "CLI Reference",
        link: "/cli",
        items: commands.map((cmd) => ({
          text: cmd.join(" "),
          link: `/cli/${cmd.join("/")}`,
        })),
      },
    ],

    socialLinks: [{ icon: "github", link: "https://github.com/jdx/pitchfork" }],

    logo: "/img/logo.png",

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
