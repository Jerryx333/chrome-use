import { sendApns } from "./apns";
import { redact } from "./encoding";
import { sendFcm } from "./fcm";
import { RETRY_AFTER_SECONDS, allow } from "./ratelimit";
import { signRelayToken, verifyRelayToken, type ApnsEnv, type Platform } from "./token";
import type { Env, Priority, PushMessage, SendResult } from "./types";

export type { Env } from "./types";

/** Whole request body cap; checked before JSON parsing. */
export const MAX_BODY_BYTES = 16 * 1024;
/** Cap on the base64 `payload` string (the spec's 2 KiB), measured encoded. */
export const MAX_PAYLOAD_CHARS = 2048;
/** Smallest valid payload: 4-byte kid + 12-byte nonce + 16-byte GCM tag. */
export const MIN_PAYLOAD_BYTES = 32;

const BASE64_RE = /^[A-Za-z0-9+/]+={0,2}$/;
const APNS_TOKEN_RE = /^[0-9a-fA-F]{64,200}$/;
const FCM_TOKEN_RE = /^[A-Za-z0-9_:.\-]{20,4096}$/;
const COLLAPSE_ID_RE = /^[A-Za-z0-9._:\-]{1,64}$/;

function json(status: number, body: Record<string, unknown>, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store", ...headers },
  });
}

const badRequest = (reason: string) => json(400, { status: "bad_request", reason });
const rateLimited = () =>
  json(429, { status: "rate_limited", retryAfter: RETRY_AFTER_SECONDS }, { "retry-after": String(RETRY_AFTER_SECONDS) });

async function readJson(request: Request): Promise<Record<string, unknown> | Response> {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_BODY_BYTES) return json(413, { status: "too_large" });
  let text: string;
  try {
    const buf = await request.arrayBuffer();
    if (buf.byteLength > MAX_BODY_BYTES) return json(413, { status: "too_large" });
    text = new TextDecoder().decode(buf);
  } catch {
    return badRequest("unreadable body");
  }
  try {
    const parsed = JSON.parse(text);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return badRequest("body must be an object");
    return parsed as Record<string, unknown>;
  } catch {
    return badRequest("invalid json");
  }
}

function clientIp(request: Request): string {
  return request.headers.get("cf-connecting-ip") ?? "unknown";
}

const nowSeconds = () => Math.floor(Date.now() / 1000);

/** Decoded length of a base64 string, computed without decoding it. */
function base64DecodedLength(b64: string): number {
  const padding = b64.endsWith("==") ? 2 : b64.endsWith("=") ? 1 : 0;
  return (b64.length / 4) * 3 - padding;
}

/** Rate-limit key for one physical device: tokens are free to mint, devices are not. */
async function deviceKey(platform: Platform, deviceToken: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`${platform}:${deviceToken}`));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

async function handleRegister(request: Request, env: Env): Promise<Response> {
  if (!(await allow(env.RL_REGISTER, clientIp(request)))) return rateLimited();
  const body = await readJson(request);
  if (body instanceof Response) return body;

  const platform = body.platform as Platform;
  const deviceToken = body.deviceToken;
  if (platform !== "ios" && platform !== "android") return badRequest("platform");
  if (typeof deviceToken !== "string") return badRequest("deviceToken");
  if (!(platform === "ios" ? APNS_TOKEN_RE : FCM_TOKEN_RE).test(deviceToken)) return badRequest("deviceToken");
  let apnsEnv: ApnsEnv = "production";
  if (body.env !== undefined) {
    if (body.env !== "sandbox" && body.env !== "production") return badRequest("env");
    apnsEnv = body.env;
  }

  const { relayToken, claims } = await signRelayToken(env.RELAY_SECRET!, platform, deviceToken, apnsEnv, nowSeconds());
  return json(200, { relayToken, expiresAt: new Date(claims.exp * 1000).toISOString() });
}

async function handlePush(request: Request, env: Env): Promise<Response> {
  if (!(await allow(env.RL_IP, clientIp(request)))) return rateLimited();
  const body = await readJson(request);
  if (body instanceof Response) return body;

  const verified = await verifyRelayToken(env.RELAY_SECRET!, body.relayToken, nowSeconds());
  if (!verified.ok) return json(401, { status: verified.status });
  // Keyed by a hash of the device, so minting many relayTokens for one device does not
  // multiply its budget, and device tokens stay out of limiter keys.
  if (!(await allow(env.RL_TOKEN, await deviceKey(verified.claims.p, verified.claims.d)))) return rateLimited();

  const payload = body.payload;
  if (
    typeof payload !== "string" ||
    payload.length % 4 !== 0 ||
    !BASE64_RE.test(payload) ||
    base64DecodedLength(payload) < MIN_PAYLOAD_BYTES
  ) {
    return badRequest("payload");
  }
  if (payload.length > MAX_PAYLOAD_CHARS) return json(413, { status: "too_large" });
  const priority = body.priority as Priority;
  if (priority !== "high" && priority !== "normal") return badRequest("priority");
  const collapseId = body.collapseId;
  if (collapseId !== undefined && (typeof collapseId !== "string" || !COLLAPSE_ID_RE.test(collapseId))) {
    return badRequest("collapseId");
  }

  const { claims } = verified;
  const message: PushMessage = { deviceToken: claims.d, payload, priority, collapseId };
  const result: SendResult =
    claims.p === "ios" ? await sendApns(env, claims.e, message, nowSeconds()) : await sendFcm(env, message, nowSeconds());

  switch (result.kind) {
    case "sent":
      return json(200, { status: "sent" });
    case "unregistered":
      console.log(`push unregistered ${claims.p} ${redact(claims.d)}: ${result.detail}`);
      return json(410, { status: "unregistered", reason: result.reason });
    case "upstream_error":
      console.log(`push upstream error ${claims.p} ${redact(claims.d)}: ${result.detail}`);
      return json(502, result.reason ? { status: "upstream_error", reason: result.reason } : { status: "upstream_error" });
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (pathname !== "/v1/register" && pathname !== "/v1/push") return json(404, { status: "not_found" });
    if (request.method !== "POST") return json(405, { status: "method_not_allowed" }, { allow: "POST" });
    if (!env.RELAY_SECRET || env.RELAY_SECRET.length < 32) {
      console.log("RELAY_SECRET missing or shorter than 32 chars");
      return json(500, { status: "misconfigured" });
    }
    return pathname === "/v1/register" ? handleRegister(request, env) : handlePush(request, env);
  },
};
