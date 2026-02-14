// docs/settings.data.ts
// Data loader for settings.toml - parses the schema and renders docs to HTML
import * as fs from "node:fs";
import * as path from "node:path";

// Simple TOML parser for settings.toml
function parseToml(content: string): Record<string, any> {
  const result: Record<string, any> = {};
  let currentSection: string[] = [];

  const lines = content.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();

    // Skip comments and empty lines
    if (line.startsWith("#") || line === "") continue;

    // Section header
    if (line.startsWith("[") && line.endsWith("]")) {
      currentSection = line.slice(1, -1).split(".");
      // Create nested structure
      let obj = result;
      for (const key of currentSection) {
        if (!obj[key]) obj[key] = {};
        obj = obj[key];
      }
      continue;
    }

    // Key-value pair
    const eqIndex = line.indexOf("=");
    if (eqIndex > 0) {
      const key = line.slice(0, eqIndex).trim();
      let value: string | boolean = line.slice(eqIndex + 1).trim();

      // Handle multi-line strings (""")
      if (value.startsWith('"""')) {
        const multiLineContent: string[] = [];
        if (value.length > 3 && value.endsWith('"""')) {
          // Single-line triple-quoted string
          value = value.slice(3, -3);
        } else {
          // Multi-line string
          i++;
          while (i < lines.length && !lines[i].trim().endsWith('"""')) {
            multiLineContent.push(lines[i]);
            i++;
          }
          value = multiLineContent.join("\n");
        }
      } else if (value.startsWith('"') && value.endsWith('"')) {
        // Regular string - remove outer quotes and unescape inner quotes
        value = value.slice(1, -1).replace(/\\"/g, '"');
      } else if (value === "true") {
        // Boolean true
        value = "true";
      } else if (value === "false") {
        // Boolean false
        value = "false";
      }
      // Numbers are kept as strings for simplicity in this parser

      // Navigate to current section
      let obj = result;
      for (const sectionKey of currentSection) {
        obj = obj[sectionKey];
      }
      obj[key] = value;
    }
  }

  return result;
}

interface SettingDef {
  type?: string;
  default?: string;
  env?: string;
  description?: string;
  docs?: string;
  docsHtml?: string;
  [key: string]: any;
}

interface SettingsData {
  [key: string]: SettingDef | SettingsData;
}

// Convert markdown to HTML using marked
async function markdownToHtml(md: string): Promise<string> {
  if (!md) return "";
  const { marked } = await import("marked");
  return marked.parse(md, { async: false }) as string;
}

// Process the docs field for each setting
async function processDocs(obj: SettingsData): Promise<void> {
  for (const key in obj) {
    const value = obj[key];
    if (typeof value !== "object" || value === null) continue;

    if (value.type) {
      // This is a leaf setting
      const content = (value.docs || value.description || "") as string;
      value.docsHtml = await markdownToHtml(content);
    } else {
      // Nested group
      await processDocs(value as SettingsData);
    }
  }
}

// Flatten settings for easier iteration
interface FlatSetting {
  name: string;
  section: string;
  type: string;
  default: string;
  env: string;
  description: string;
  docsHtml: string;
}

function flattenSettings(
  obj: SettingsData,
  prefix: string = ""
): FlatSetting[] {
  const result: FlatSetting[] = [];

  for (const key in obj) {
    const value = obj[key];
    if (typeof value !== "object" || value === null) continue;

    const fullName = prefix ? `${prefix}.${key}` : key;

    if (value.type) {
      // Leaf setting
      result.push({
        name: key,
        section: prefix,
        type: (value.type as string) || "String",
        default: (value.default as string) || "",
        env: (value.env as string) || "",
        description: (value.description as string) || "",
        docsHtml: (value.docsHtml as string) || "",
      });
    } else {
      // Nested group - recurse
      result.push(...flattenSettings(value as SettingsData, fullName));
    }
  }

  return result;
}

// Group settings by section
interface GroupedSettings {
  [section: string]: FlatSetting[];
}

function groupBySection(settings: FlatSetting[]): GroupedSettings {
  const grouped: GroupedSettings = {};

  for (const setting of settings) {
    if (!grouped[setting.section]) {
      grouped[setting.section] = [];
    }
    grouped[setting.section].push(setting);
  }

  return grouped;
}

export default {
  watch: ["settings.toml"],

  async load() {
    const settingsPath = path.resolve(__dirname, "../settings.toml");
    const raw = fs.readFileSync(settingsPath, "utf-8");
    const data = parseToml(raw) as SettingsData;

    // Process markdown docs to HTML
    await processDocs(data);

    // Flatten and group for easy rendering
    const flat = flattenSettings(data);
    const grouped = groupBySection(flat);

    return {
      raw: data,
      flat,
      grouped,
      sections: Object.keys(grouped),
    };
  },
};
