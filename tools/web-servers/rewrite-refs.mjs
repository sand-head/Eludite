// Rewrite the `$ref`s of the cached SchemaStore schemas that name another cached schema by its SchemaStore URL
// (https://www.schemastore.org/<file> or https://json.schemastore.org/<file>) to that copy's file URI under `final`,
// the folder the schemas end up in. Run by fetch.sh and fetch.ps1 (brief 0050): node rewrite-refs.mjs <final> <stage>.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const [finalDir, stageDir] = process.argv.slice(2);
const files = new Set(readdirSync(stageDir).filter((f) => f.endsWith(".json")));
const rewrite = (value) => {
  if (Array.isArray(value)) return value.map(rewrite);
  if (value && typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) {
      const m = k === "$ref" && typeof v === "string" && v.match(/^https:\/\/(?:www|json)\.schemastore\.org\/([^#]+)(#.*)?$/);
      out[k] = m && files.has(m[1]) ? pathToFileURL(join(finalDir, m[1])).href + (m[2] ?? "") : rewrite(v);
    }
    return out;
  }
  return value;
};
for (const f of files) {
  const path = join(stageDir, f);
  writeFileSync(path, JSON.stringify(rewrite(JSON.parse(readFileSync(path, "utf8"))), null, 2));
}
