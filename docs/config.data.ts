// docs/config.data.ts
// Data loader for config.toml - parses the daemon configuration schema
import * as fs from "node:fs";
import * as path from "node:path";

// Simple TOML parser for config.toml
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
      let value: string = line.slice(eqIndex + 1).trim();

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
      }

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

interface ConfigOption {
  type: string;
  required?: string;
  default?: string;
  description: string;
  docs?: string;
  docsHtml?: string;
  example?: string;
}

// Convert markdown to HTML using marked
async function markdownToHtml(md: string): Promise<string> {
  if (!md) return "";
  const { marked } = await import("marked");
  return marked.parse(md, { async: false }) as string;
}

// Process the docs field for each option (including nested configs)
async function processDocs(
  data: Record<string, ConfigOption | Record<string, ConfigOption>>
): Promise<void> {
  for (const key in data) {
    const value = data[key];
    if (typeof value !== "object" || value === null) continue;

    // Check if this is a leaf option (has "type" field) or nested group
    if ("type" in value && typeof value.type === "string") {
      const option = value as ConfigOption;
      const content = option.docs || option.description || "";
      option.docsHtml = await markdownToHtml(content);
    } else {
      // Nested group - recurse
      await processDocs(value as Record<string, ConfigOption>);
    }
  }
}

interface FlatOption {
  name: string;
  type: string;
  required: boolean;
  default: string;
  description: string;
  docsHtml: string;
  example: string;
}

function flattenOptions(
  data: Record<string, ConfigOption | Record<string, ConfigOption>>,
  prefix: string = ""
): FlatOption[] {
  const result: FlatOption[] = [];

  for (const key in data) {
    const value = data[key];
    if (typeof value !== "object" || value === null) continue;

    const fullName = prefix ? `${prefix}.${key}` : key;

    // Check if this is a leaf option (has "type" field) or nested group
    if ("type" in value && typeof value.type === "string") {
      const option = value as ConfigOption;
      result.push({
        name: fullName,
        type: option.type || "String",
        required: option.required === "true",
        default: option.default || "",
        description: option.description || "",
        docsHtml: option.docsHtml || "",
        example: option.example || "",
      });
    } else {
      // Nested group - recurse
      result.push(
        ...flattenOptions(value as Record<string, ConfigOption>, fullName)
      );
    }
  }

  return result;
}

export default {
  watch: ["config.toml"],

  async load() {
    const configPath = path.resolve(__dirname, "../config.toml");
    const raw = fs.readFileSync(configPath, "utf-8");
    const data = parseToml(raw) as Record<
      string,
      ConfigOption | Record<string, ConfigOption>
    >;

    // Process markdown docs to HTML
    await processDocs(data);

    // Flatten for easy rendering
    const options = flattenOptions(data);

    return {
      raw: data,
      options,
    };
  },
};
