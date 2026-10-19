import { base64urlEncode, pemToDer, utf8 } from "./encoding";
import type { Env, PushMessage, SendResult } from "./types";

const FCM_SCOPE = "https://www.googleapis.com/auth/firebase.messaging";
const DEFAULT_TOKEN_URI = "https://oauth2.googleapis.com/token";
const ACCESS_TOKEN_MARGIN_SECONDS = 300;

interface ServiceAccount {
  project_id: string;
  client_email: string;
  private_key: string;
  token_uri: string;
}

interface CachedAccessToken {
  cacheKey: string;
  token: string;
  expiresAt: number;
}

let cachedAccessToken: CachedAccessToken | null = null;
let parsedAccount: { raw: string; account: ServiceAccount } | null = null;

export function resetFcmCache(): void {
  cachedAccessToken = null;
  parsedAccount = null;
}

function serviceAccount(env: Env): ServiceAccount | null {
  const raw = env.FCM_SERVICE_ACCOUNT_JSON;
  if (!raw) return null;
  if (parsedAccount && parsedAccount.raw === raw) return parsedAccount.account;
  try {
    const json = JSON.parse(raw) as Partial<ServiceAccount>;
    if (!json.project_id || !json.client_email || !json.private_key) return null;
    const account: ServiceAccount = {
      project_id: json.project_id,
      client_email: json.client_email,
      private_key: json.private_key,
      token_uri: json.token_uri || DEFAULT_TOKEN_URI,
    };
    parsedAccount = { raw, account };
    return account;
  } catch {
    return null;
  }
}

async function accessToken(account: ServiceAccount, nowSeconds: number): Promise<string | null> {
  const cacheKey = `${account.client_email}:${account.project_id}`;
  if (cachedAccessToken && cachedAccessToken.cacheKey === cacheKey && nowSeconds < cachedAccessToken.expiresAt) {
    return cachedAccessToken.token;
  }
  const key = await crypto.subtle.importKey(
    "pkcs8",
    pemToDer(account.private_key),
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const header = base64urlEncode(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const claims = base64urlEncode(
    JSON.stringify({
      iss: account.client_email,
      scope: FCM_SCOPE,
      aud: account.token_uri,
      iat: nowSeconds,
      exp: nowSeconds + 3600,
    }),
  );
  const signingInput = `${header}.${claims}`;
  const sig = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", key, utf8(signingInput));
  const assertion = `${signingInput}.${base64urlEncode(sig)}`;

  const res = await fetch(account.token_uri, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
      assertion,
    }).toString(),
  });
  if (!res.ok) return null;
  const body = (await res.json()) as { access_token?: unknown; expires_in?: unknown };
  if (typeof body.access_token !== "string") return null;
  const expiresIn = typeof body.expires_in === "number" ? body.expires_in : 3600;
  cachedAccessToken = {
    cacheKey,
    token: body.access_token,
    expiresAt: nowSeconds + Math.max(60, expiresIn - ACCESS_TOKEN_MARGIN_SECONDS),
  };
  return body.access_token;
}

export function fcmBody(message: PushMessage): string {
  const android: Record<string, string> = { priority: message.priority === "high" ? "HIGH" : "NORMAL" };
  if (message.collapseId) android.collapse_key = message.collapseId;
  return JSON.stringify({
    message: {
      token: message.deviceToken,
      data: { e: message.payload },
      android,
    },
  });
}

interface FcmError {
  error?: {
    status?: string;
    details?: Array<{ errorCode?: string }>;
  };
}

export async function sendFcm(env: Env, message: PushMessage, nowSeconds: number): Promise<SendResult> {
  const account = serviceAccount(env);
  if (!account) return { kind: "upstream_error", detail: "fcm not configured" };
  let token: string | null;
  try {
    token = await accessToken(account, nowSeconds);
  } catch {
    return { kind: "upstream_error", detail: "fcm auth failed" };
  }
  if (!token) return { kind: "upstream_error", detail: "fcm auth rejected" };

  let res: Response;
  try {
    res = await fetch(
      `https://fcm.googleapis.com/v1/projects/${encodeURIComponent(account.project_id)}/messages:send`,
      {
        method: "POST",
        headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
        body: fcmBody(message),
      },
    );
  } catch {
    return { kind: "upstream_error", detail: "fcm fetch failed" };
  }
  if (res.ok) return { kind: "sent" };

  let status = "";
  let errorCode = "";
  try {
    const parsed = (await res.json()) as FcmError;
    status = parsed.error?.status?.slice(0, 64) ?? "";
    errorCode = parsed.error?.details?.find((d) => typeof d.errorCode === "string")?.errorCode?.slice(0, 64) ?? "";
  } catch {
    // Non-JSON error body: plain upstream error.
  }
  const detail = `fcm ${res.status} ${errorCode || status}`.trim();
  if (errorCode === "UNREGISTERED" || status === "NOT_FOUND" || res.status === 404) {
    return { kind: "unregistered", detail };
  }
  if (res.status === 401) cachedAccessToken = null;
  return { kind: "upstream_error", detail };
}
