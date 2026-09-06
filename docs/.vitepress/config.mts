import { socialCard, writeSocialCard } from "./social-images.mjs";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type DefaultTheme } from "vitepress";

import spec from "../cli/commands.json";

interface Cmd {
  name: string;
  full_cmd: string[];
  subcommands: Record<string, Cmd>;
  hide?: boolean;
}

/** Build nested CLI navigation from the visible commands in the generated spec. */
function commandSidebar(cmd: Cmd): DefaultTheme.SidebarItem[] {
  return Object.values(cmd.subcommands)
    .filter((sub) => !sub.hide)
    .map((sub) => {
      const children = commandSidebar(sub);
      return {
        text: sub.name,
        link: `/cli/${sub.full_cmd.join("/")}`,
        ...(children.length ? { collapsed: true, items: children } : {}),
      };
    });
}

const configDir = dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(resolve(configDir, "../../Cargo.toml"), "utf8");
const versionMatch = cargoToml.match(
  /^\[package\][\s\S]*?^\s*version\s*=\s*"([^"]+)"/m,
);
if (!versionMatch) {
  console.warn("Unable to find package version in Cargo.toml");
}
const latestVersion = versionMatch?.[1] ?? "0.0.0";
const siteUrl = "https://pitchfork.jdx.dev";
const siteDescription =
  "Run and supervise long-lived development processes with automatic restarts, ready checks, file watching, schedules, logs, and terminal or web dashboards.";

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "pitchfork",
  description: siteDescription,
  // Default to dark while allowing readers to choose a light theme.
  appearance: "dark",
  sitemap: { hostname: siteUrl },
  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: "Get started", link: "/quickstart" },
      { text: "Guides", link: "/guides/" },
      { text: "Reference", link: "/reference/configuration" },
      {
        text: `v${latestVersion}`,
        link: "https://github.com/jdx/pitchfork/releases",
      },
    ],

    sidebar: [
      {
        text: "Get started",
        items: [
          { text: "Quickstart", link: "/quickstart" },
          { text: "Installation", link: "/installation" },
          { text: "Your first project", link: "/first-daemon" },
          { text: "How pitchfork works", link: "/concepts/how-it-works" },
        ],
      },
      {
        text: "Run your project",
        items: [
          { text: "Guide index", link: "/guides/" },
          { text: "Shell hooks & sessions", link: "/guides/shell-hook" },
          { text: "Ready checks", link: "/guides/ready-checks" },
          { text: "File watching", link: "/guides/file-watching" },
          { text: "Automatic retries", link: "/guides/auto-restart" },
          { text: "Health checks", link: "/guides/health-checks" },
          { text: "Ports & local URLs", link: "/guides/port-management" },
          { text: "Namespaces & worktrees", link: "/concepts/namespaces" },
        ],
      },
      {
        text: "Observe & troubleshoot",
        items: [
          { text: "Logs", link: "/guides/logs" },
          { text: "Terminal dashboard", link: "/guides/tui" },
          { text: "Web dashboard", link: "/guides/web-ui" },
          { text: "Troubleshooting", link: "/troubleshooting" },
        ],
      },
      {
        text: "Automate & integrate",
        collapsed: true,
        items: [
          { text: "mise integration", link: "/guides/mise-integration" },
          {
            text: "Configuration templates",
            link: "/guides/configuration-templates",
          },
          { text: "Lifecycle hooks", link: "/guides/lifecycle-hooks" },
          { text: "Cron scheduling", link: "/guides/scheduling" },
          { text: "Login & boot", link: "/guides/boot-start" },
          { text: "MCP server", link: "/guides/mcp" },
          { text: "Container mode", link: "/guides/container-mode" },
        ],
      },
      {
        text: "Reference",
        collapsed: false,
        items: [
          { text: "Daemon configuration", link: "/reference/configuration" },
          { text: "Settings", link: "/reference/settings" },
          {
            text: "Environment variables",
            link: "/reference/environment-vars",
          },
          { text: "File locations", link: "/reference/file-locations" },
          { text: "HTTP API", link: "/reference/http-api" },
          {
            text: "CLI reference",
            link: "/cli/",
            collapsed: true,
            items: commandSidebar(spec.cmd),
          },
        ],
      },
      {
        text: "Contribute",
        collapsed: true,
        items: [
          { text: "Development guide", link: "/contributing" },
          { text: "Architecture", link: "/concepts/architecture" },
        ],
      },
    ],

    outline: {
      level: [2, 3],
    },

    socialLinks: [{ icon: "github", link: "https://github.com/jdx/pitchfork" }],

    logo: "/img/android-chrome-192x192.png",

    footer: false,

    editLink: {
      pattern: "https://github.com/jdx/pitchfork/edit/main/docs/:path",
      text: "Edit this page on GitHub",
    },

    search: {
      provider: "local",
    },
  },
  head: [
    [
      "script",
      {},
      `(function () {
  try {
    var d = document.documentElement;
    var c = JSON.parse(localStorage.getItem("jdx-banner-cache") || "null");
    var expires = c && c.expires ? Date.parse(c.expires) : NaN;
    var now = Date.now();
    var metadataValid =
      c &&
      typeof c.id === "string" &&
      typeof c.height === "string" &&
      /^[1-9]\\d*(?:\\.\\d+)?px$/.test(c.height) &&
      Number.isFinite(c.width) &&
      typeof c.fontSize === "string" &&
      Number.isFinite(c.pixelRatio) &&
      Number.isFinite(c.cachedAt) &&
      c.cachedAt <= now &&
      now - c.cachedAt < 300000 &&
      (!c.expires || (typeof c.expires === "string" && Number.isFinite(expires) && now < expires));
    var contextMatches =
      metadataValid &&
      c.width === innerWidth &&
      c.fontSize === getComputedStyle(d).fontSize &&
      c.pixelRatio === devicePixelRatio;
    if (contextMatches && localStorage.getItem("jdx-banner-dismissed") !== c.id)
      d.style.setProperty("--vp-layout-top-height", c.height);
    else if (c && !metadataValid)
      localStorage.removeItem("jdx-banner-cache");
  } catch (e) {}
})();`,
    ],
    ["link", { rel: "icon", href: "/img/favicon.ico", sizes: "any" }],
    [
      "link",
      {
        rel: "icon",
        href: "/img/favicon-32x32.png",
        type: "image/png",
        sizes: "32x32",
      },
    ],
    ["link", { rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }],
    [
      "link",
      {
        rel: "apple-touch-icon",
        href: "/img/apple-touch-icon.png",
        sizes: "180x180",
      },
    ],
    ["link", { rel: "manifest", href: "/site.webmanifest" }],
    ["meta", { name: "theme-color", content: "#dc2626" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:locale", content: "en_US" }],
    ["meta", { property: "og:site_name", content: "pitchfork" }],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { name: "twitter:site", content: "@jdxcode" }],
  ],

  transformPageData(pageData) {
    if (pageData.relativePath.startsWith("cli/")) {
      pageData.frontmatter.editLink = false;
    }
  },

  transformHead({ pageData, title, description, siteConfig }) {
    const heading =
      pageData.relativePath === "index.md"
        ? "Daemons with DX"
        : pageData.title || "pitchfork";
    const card = socialCard(heading);
    writeSocialCard(siteConfig.outDir, card);
    const image = new URL(card.path, `${siteUrl}/`).toString();
    const imageAlt = `${heading} — pitchfork docs`;
    const url = `${siteUrl}/${pageData.relativePath}`
      .replace(/index\.md$/, "")
      .replace(/\.md$/, ".html");

    return [
      ["link", { rel: "canonical", href: url }],
      ["meta", { property: "og:url", content: url }],
      ["meta", { property: "og:image", content: image }],
      ["meta", { property: "og:image:alt", content: imageAlt }],
      ["meta", { name: "twitter:image", content: image }],
      ["meta", { name: "twitter:image:alt", content: imageAlt }],
      ["meta", { property: "og:title", content: title }],
      ["meta", { property: "og:description", content: description }],
      ["meta", { name: "twitter:title", content: title }],
      ["meta", { name: "twitter:description", content: description }],
      [
        "script",
        { type: "application/ld+json" },
        JSON.stringify({
          "@context": "https://schema.org",
          "@type": "WebPage",
          name: title,
          description,
          url,
          isPartOf: { "@type": "WebSite", name: "pitchfork", url: siteUrl },
        }),
      ],
    ];
  },

  // Ignore localhost URLs in CLI examples
  ignoreDeadLinks: [/^http:\/\/localhost/, /^http:\/\/127\.0\.0\.1/],
});
