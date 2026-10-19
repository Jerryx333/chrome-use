import { base64urlDecode, base64urlDecodeText, base64urlEncode, utf8 } from "./encoding";

export type Platform = "ios" | "android";
export type ApnsEnv = "sandbox" | "production";

/** Claims carried inside a relayToken. Short keys keep the token small. */
export interface RelayClaims {
  p: Platform;
  d: string; // device token
  e: ApnsEnv;
  iat: number; // seconds
  exp: number; // seconds
}

export const TOKEN_TTL_SECONDS = 90 * 24 * 60 * 60;
export const MAX_TOKEN_LENGTH = 8192;
const MAX_FUTURE_SKEW_SECONDS = 300;

export type VerifyResult =
  | { ok: true; claims: RelayClaims; signature: string }
  | { ok: false; status: "invalid_token" | "expired_token" };

const keyCache = new Map<string, Promise<CryptoKey>>();

function hmacKey(secret: string): Promise<CryptoKey> {
  let key = keyCache.get(secret);
  if (!key) {
    key = crypto.subtle.importKey("raw", utf8(secret), { name: "HMAC", hash: "SHA-256" }, false, [
      "sign",
      "verify",
    ]);
    keyCache.clear();
    keyCache.set(secret, key);
  }
  return key;
}

export async function signRelayToken(
  secret: string,
  platform: Platform,
  deviceToken: string,
  env: ApnsEnv,
  nowSeconds: number,
): Promise<{ relayToken: string; claims: RelayClaims }> {
  const claims: RelayClaims = {
    p: platform,
    d: deviceToken,
    e: env,
    iat: nowSeconds,
    exp: nowSeconds + TOKEN_TTL_SECONDS,
  };
  const body = base64urlEncode(JSON.stringify(claims));
  const mac = await crypto.subtle.sign("HMAC", await hmacKey(secret), utf8(body));
  return { relayToken: `${body}.${base64urlEncode(mac)}`, claims };
}

export async function verifyRelayToken(
  secret: string,
  token: unknown,
  nowSeconds: number,
): Promise<VerifyResult> {
  const invalid = { ok: false, status: "invalid_token" } as const;
  if (typeof token !== "string" || token.length === 0 || token.length > MAX_TOKEN_LENGTH) return invalid;
  const parts = token.split(".");
  if (parts.length !== 2) return invalid;
  const [body, sig] = parts;
  const mac = base64urlDecode(sig);
  if (!mac || mac.length !== 32) return invalid;
  // subtle.verify compares the MAC in constant time.
  const valid = await crypto.subtle.verify("HMAC", await hmacKey(secret), mac, utf8(body));
  if (!valid) return invalid;

  const json = base64urlDecodeText(body);
  if (json === null) return invalid;
  let claims: RelayClaims;
  try {
    claims = JSON.parse(json);
  } catch {
    return invalid;
  }
  if (
    !claims ||
    (claims.p !== "ios" && claims.p !== "android") ||
    typeof claims.d !== "string" ||
    (claims.e !== "sandbox" && claims.e !== "production") ||
    !Number.isInteger(claims.iat) ||
    !Number.isInteger(claims.exp)
  ) {
    return invalid;
  }
  if (claims.iat > nowSeconds + MAX_FUTURE_SKEW_SECONDS) return invalid;
  if (claims.exp <= nowSeconds) return { ok: false, status: "expired_token" };
  return { ok: true, claims, signature: sig };
}
