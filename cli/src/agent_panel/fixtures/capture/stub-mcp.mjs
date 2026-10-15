// Minimal stdio MCP server named chrome_use for protocol fixture capture.
// chrome_use_read returns the action marker as its first text block plus
// structuredContent; chrome_use_shot returns a marker and a 1x1 PNG image.
// Every request and response is appended to $STUB_MCP_LOG when set.
// STUB_MCP_NO_STRUCTURED=1 omits structuredContent; STUB_MCP_OUTPUT_SCHEMA=1
// declares an outputSchema on chrome_use_read.
import { appendFileSync } from "node:fs";
import { createInterface } from "node:readline";

const READ_ID = "11111111-2222-4333-8444-555555555555";
const SHOT_ID = "99999999-8888-4777-8666-555555555555";
const PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
const log = (direction, value) => {
  if (process.env.STUB_MCP_LOG)
    appendFileSync(process.env.STUB_MCP_LOG, JSON.stringify({ direction, value }) + "\n");
};
const send = (value) => {
  log("out", value);
  process.stdout.write(JSON.stringify(value) + "\n");
};
const structured = (id) =>
  process.env.STUB_MCP_NO_STRUCTURED === "1" ? {} : { structuredContent: { chromeUseAction: { actionId: id } } };
const marker = (id) => `chrome-use action ${id}`;
const schema = {
  type: "object",
  properties: {
    chromeUseAction: {
      type: "object",
      properties: { actionId: { type: "string" } },
      required: ["actionId"],
    },
  },
  required: ["chromeUseAction"],
};
const tools = [
  {
    name: "chrome_use_read",
    description: "Read the current page text. Returns a chrome-use action marker.",
    inputSchema: { type: "object", properties: {} },
    ...(process.env.STUB_MCP_OUTPUT_SCHEMA === "1" ? { outputSchema: schema } : {}),
  },
  {
    name: "chrome_use_shot",
    description: "Take a screenshot of the current page. Returns an image.",
    inputSchema: { type: "object", properties: {} },
  },
];

createInterface({ input: process.stdin }).on("line", (line) => {
  if (!line.trim()) return;
  const message = JSON.parse(line);
  log("in", message);
  const { id, method, params } = message;
  if (id === undefined) return;
  if (method === "initialize")
    return send({
      jsonrpc: "2.0",
      id,
      result: {
        protocolVersion: params?.protocolVersion ?? "2025-06-18",
        capabilities: { tools: {} },
        serverInfo: { name: "chrome_use", version: "0.0.0-fixture" },
      },
    });
  if (method === "ping") return send({ jsonrpc: "2.0", id, result: {} });
  if (method === "tools/list") return send({ jsonrpc: "2.0", id, result: { tools } });
  if (method === "tools/call" && params?.name === "chrome_use_read")
    return send({
      jsonrpc: "2.0",
      id,
      result: {
        content: [
          { type: "text", text: marker(READ_ID) },
          { type: "text", text: "Page title: Fixture page. Body: hello from the stub." },
        ],
        ...structured(READ_ID),
        isError: false,
      },
    });
  if (method === "tools/call" && params?.name === "chrome_use_shot")
    return send({
      jsonrpc: "2.0",
      id,
      result: {
        content: [
          { type: "text", text: marker(SHOT_ID) },
          { type: "image", data: PNG, mimeType: "image/png" },
        ],
        ...structured(SHOT_ID),
        isError: false,
      },
    });
  send({ jsonrpc: "2.0", id, error: { code: -32601, message: `unsupported ${method}` } });
});
