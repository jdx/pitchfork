// docs/config.data.ts
// Data loader for config.toml - parses the daemon configuration schema
import * as fs from "node:fs";
import * as path from "node:path";
import { parse as parseToml } from "smol-toml";

interface ConfigOption {
  type: string;
  required?: boolean | string;
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
        required: option.required === true || option.required === "true",
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
