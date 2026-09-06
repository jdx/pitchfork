// Check copyable examples before building and local links after rendering.
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import { dirname, extname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { marked } from "marked";
import { parse as parseToml } from "smol-toml";

const docs = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const walk = (dir) =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name.startsWith(".") || entry.name === "node_modules") return [];
    const path = join(dir, entry.name);
    return entry.isDirectory() ? walk(path) : [path];
  });
const errors = new Set();
const report = (file, message) =>
  errors.add(`${relative(docs, file)}: ${message}`);

if (process.argv.includes("--examples")) {
  const files = [
    ...walk(docs).filter((file) => file.endsWith(".md")),
    resolve(docs, "../README.md"),
  ];
  let examples = 0;
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    marked.walkTokens(marked.lexer(source), (token) => {
      if (token.type !== "code") return;
      const language = token.lang?.split(/\s/)[0];
      if (language !== "toml" && language !== "json") return;
      examples++;
      try {
        if (language === "toml") parseToml(token.text);
        else JSON.parse(token.text);
      } catch (error) {
        const line = source
          .slice(0, source.indexOf(token.raw))
          .split("\n").length;
        report(
          file,
          `invalid ${language} example at line ${line}: ${error.message}`,
        );
      }
    });
  }
  console.log(
    `Checked ${examples} TOML/JSON examples in ${files.length} Markdown files.`,
  );
} else {
  const dist = resolve(docs, ".vitepress/dist");
  const pages = walk(dist).filter((file) => file.endsWith(".html"));
  const contents = new Map(
    pages.map((file) => [file, readFileSync(file, "utf8")]),
  );
  const ids = new Map(
    [...contents].map(([file, html]) => [
      file,
      new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1])),
    ]),
  );
  let links = 0;
  for (const [file, html] of contents) {
    for (const match of html.matchAll(
      /<(a|img)\b[^>]*?\b(?:href|src)="([^"]+)"/g,
    )) {
      const href = match[2].replaceAll("&amp;", "&");
      if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(href)) continue;
      links++;
      const currentPath = `/${relative(dist, file).replaceAll("\\", "/")}`;
      const url = new URL(href, `https://docs.example${currentPath}`);
      let target = resolve(dist, `.${decodeURIComponent(url.pathname)}`);
      if (existsSync(target) && statSync(target).isDirectory())
        target = join(target, "index.html");
      else if (!extname(target)) {
        target = existsSync(`${target}.html`)
          ? `${target}.html`
          : join(target, "index.html");
      }
      if (!existsSync(target)) {
        report(file, `missing local target ${href}`);
      } else if (url.hash && ids.has(target)) {
        const anchor = decodeURIComponent(url.hash.slice(1));
        if (anchor && !ids.get(target).has(anchor))
          report(file, `missing anchor ${href}`);
      }
    }
  }
  console.log(
    `Checked ${links} local links and images across ${pages.length} rendered pages.`,
  );
}

if (errors.size) {
  console.error([...errors].join("\n"));
  process.exitCode = 1;
}
