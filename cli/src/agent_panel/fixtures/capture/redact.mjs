// Turns a raw capture into a committed fixture: drops user hook chatter,
// trims catalogs that only reflect the capturing machine (commands, skills,
// plugins, model lists, Codex config), and redacts account identity, host
// names and home/scratch paths. Usage: node redact.mjs <in.jsonl> <out.jsonl>
import { readFileSync, writeFileSync } from "node:fs";

const [input, output] = process.argv.slice(2);
const TRIMMED = "<trimmed: capture-machine catalog>";
const trim = (object, keys) => {
  for (const key of keys) if (object && key in object) object[key] = TRIMMED;
};
const lines = [];
for (const raw of readFileSync(input, "utf8").split("\n")) {
  if (!raw.trim()) continue;
  let value;
  try { value = JSON.parse(raw); } catch { lines.push(raw); continue; }
  // User-configured hooks are not part of the protocol under test.
  if (value.type === "system" && /^hook_/.test(value.subtype ?? "")) continue;
  if (/^hook\//.test(value.method ?? "")) continue;
  if (value.type === "control_response") {
    const r = value.response?.response;
    trim(r, ["commands", "agents", "models", "available_output_styles"]);
    if (r?.account) r.account = { email: "user@example.invalid", organization: "<redacted>", subscriptionType: r.account.subscriptionType, apiProvider: r.account.apiProvider };
  }
  if (value.type === "system" && value.subtype === "init")
    trim(value, ["slash_commands", "skills", "plugins", "agents", "plugin_errors", "memory_paths"]);
  // Codex config/read reply (id 2 in drive.mjs) carries the whole user config.
  if (value.result?.config && value.id !== undefined) value.result = { config: TRIMMED };
  // ACP session/new: keep each option's current value, drop the catalog.
  for (const option of value.result?.configOptions ?? []) if (Array.isArray(option.options)) option.options = TRIMMED;
  if (value.method === "remoteControl/status/changed") {
    value.params.serverName = "host.local";
    value.params.installationId = "00000000-0000-4000-8000-000000000000";
  }
  if (value.method === "account/rateLimits/updated" && value.params?.rateLimits?.credits)
    value.params.rateLimits.credits.balance = "0";
  lines.push(JSON.stringify(value));
}
const text = lines
  .join("\n")
  .replace(/[A-Za-z0-9._%+-]+@gmail\.com/g, "user@example.invalid")
  .replace(/\/private\/tmp\/claude-[^"\\\s]*?\/scratchpad\/cap(?:190)?/g, "/WORK")
  .replace(/\/Users\/leo\/fixture-capture\/out/g, "/WORK")
  .replace(/\/Users\/leo\/fixture-capture/g, "/CAPTURE")
  .replace(/-private-tmp-claude-[^/"\\\s]*?-scratchpad-cap(?:190)?-/g, "-WORK-")
  .replace(/\/Users\/leo\b/g, "/Users/USER")
  .replace(/leos-MacBook-Pro\.local/g, "host.local");
writeFileSync(output, text + "\n");
