import { base64urlEncode, pemToDer, utf8 } from "./encoding";
import type { ApnsEnv } from "./token";
import type { Env, PushMessage, SendResult } from "./types";

export const APNS_HOSTS: Record<ApnsEnv, string> = {
  production: "https://api.push.apple.com",
  sandbox: "https://api.sandbox.push.apple.com",
};

// Apple rejects provider tokens older than 60 min and throttles refreshes faster than 20 min.
const JWT_TTL_SECONDS = 50 * 60;

const GENERIC_ALERT = { title: "chrome-use", body: "chrome-use needs you" };

interface CachedJwt {
  cacheKey: string;
  jwt: string;
  issuedAt: number;
}

let cachedJwt: CachedJwt | null = null;

export function resetApnsCache(): void {
  cachedJwt = null;
}

async function providerJwt(env: Env, nowSeconds: number): Promise<string> {
  const keyId = env.APNS_KEY_ID!;
  const teamId = env.APNS_TEAM_ID!;
  const cacheKey = `${keyId}:${teamId}:${env.APNS_KEY_P8!.length}`;
  if (cachedJwt && cachedJwt.cacheKey === cacheKey && nowSeconds - cachedJwt.issuedAt < JWT_TTL_SECONDS) {
    return cachedJwt.jwt;
  }
  const key = await crypto.subtle.importKey(
    "pkcs8",
    pemToDer(env.APNS_KEY_P8!),
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign"],
  );
  const header = base64urlEncode(JSON.stringify({ alg: "ES256", kid: keyId }));
  const claims = base64urlEncode(JSON.stringify({ iss: teamId, iat: nowSeconds }));
  const signingInput = `${header}.${claims}`;
  // WebCrypto returns raw r‖s (IEEE P1363), which is exactly the JWS ES256 encoding.
  const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, utf8(signingInput));
  const jwt = `${signingInput}.${base64urlEncode(sig)}`;
  cachedJwt = { cacheKey, jwt, issuedAt: nowSeconds };
  return jwt;
}

export function apnsConfigured(env: Env): boolean {
  return Boolean(env.APNS_KEY_P8 && env.APNS_KEY_ID && env.APNS_TEAM_ID && env.APNS_BUNDLE_ID);
}

export function apnsBody(message: PushMessage): string {
  return JSON.stringify({
    aps: {
      alert: GENERIC_ALERT,
      sound: "default",
      "mutable-content": 1,
      "interruption-level": message.priority === "high" ? "time-sensitive" : "active",
    },
    e: message.payload,
  });
}

export async function sendApns(
  env: Env,
  apnsEnv: ApnsEnv,
  message: PushMessage,
  nowSeconds: number,
): Promise<SendResult> {
  if (!apnsConfigured(env)) return { kind: "upstream_error", reason: "misconfigured", detail: "apns not configured" };
  let jwt: string;
  try {
    jwt = await providerJwt(env, nowSeconds);
  } catch {
    return { kind: "upstream_error", reason: "misconfigured", detail: "apns key import failed" };
  }
  const headers: Record<string, string> = {
    authorization: `bearer ${jwt}`,
    "apns-topic": env.APNS_BUNDLE_ID!,
    "apns-push-type": "alert",
    "apns-priority": message.priority === "high" ? "10" : "5",
    "content-type": "application/json",
  };
  if (message.collapseId) headers["apns-collapse-id"] = message.collapseId;

  let res: Response;
  try {
    res = await fetch(`${APNS_HOSTS[apnsEnv]}/3/device/${message.deviceToken}`, {
      method: "POST",
      headers,
      body: apnsBody(message),
    });
  } catch {
    return { kind: "upstream_error", detail: "apns fetch failed" };
  }
  if (res.status === 200) return { kind: "sent" };

  let reason = "";
  try {
    const parsed = (await res.json()) as { reason?: unknown };
    if (typeof parsed.reason === "string") reason = parsed.reason.slice(0, 64);
  } catch {
    // APNs error bodies are JSON; anything else is just an upstream error.
  }
  const detail = `apns ${res.status} ${reason}`.trim();
  // BadDeviceToken also covers a sandbox token sent to production (or the reverse);
  // APNs cannot tell the two apart, so the reason goes back to the caller.
  if (res.status === 410 || reason === "BadDeviceToken" || reason === "Unregistered") {
    return { kind: "unregistered", reason: reason || "Unregistered", detail };
  }
  if (reason === "ExpiredProviderToken" || reason === "InvalidProviderToken") resetApnsCache();
  return { kind: "upstream_error", detail };
}
