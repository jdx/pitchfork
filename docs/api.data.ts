// docs/api.data.ts
// Data loader for web API endpoints — parses the JSON schema exported by
// `pitchfork api-schema` and makes it available to the VitePress docs site.

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

interface ApiParam {
  name: string;
  type_name: string;
  description: string;
  required: boolean;
}

interface ApiEndpoint {
  path: string;
  method: string;
  description: string;
  auth: boolean;
  path_params?: ApiParam[];
  query_params?: ApiParam[];
  request_body?: string;
  response_type?: string;
}

interface ApiDoc {
  endpoints: ApiEndpoint[];
}

export default {
  // Rebuild when the generated schema changes.
  watch: ["./public/api-schema.json"],

  async load(): Promise<ApiDoc> {
    const schemaPath = path.resolve(__dirname, "./public/api-schema.json");

    // If the schema hasn't been generated yet (e.g. first clone), return empty.
    if (!fs.existsSync(schemaPath)) {
      console.warn(
        `[api.data.ts] api-schema.json not found at ${schemaPath}. ` +
          `Run 'pitchfork api-schema > docs/public/api-schema.json' to generate it.`
      );
      return { endpoints: [] };
    }

    const raw = fs.readFileSync(schemaPath, "utf-8");
    try {
      return JSON.parse(raw) as ApiDoc;
    } catch (e) {
      console.warn(`[api.data.ts] failed to parse api-schema.json: ${e}`);
      return { endpoints: [] };
    }
  },
};
