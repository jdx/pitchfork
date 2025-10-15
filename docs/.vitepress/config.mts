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
  description: "Daemons with DX",
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
      { text: "Configuration", link: "/config" },
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

    logo: "/logo.png",
  },
  head: [["link", { rel: "icon", href: "/img/favicon.ico" }]],
  search: {
    provider: "local",
  }
});
