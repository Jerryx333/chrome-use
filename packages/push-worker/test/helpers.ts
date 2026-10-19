import type { Env, RateLimiter } from "../src/types";

export const RELAY_SECRET = "test-relay-secret-0123456789abcdef0123456789";
export const IOS_TOKEN = "a1".repeat(32);
export const FCM_TOKEN = "fcm-token_abc:DEF-1234567890.xyz";
// 12-byte nonce + 32 bytes of "ciphertext+tag", base64 (not base64url, per spec).
export const PAYLOAD = btoa(String.fromCharCode(...Array.from({ length: 44 }, (_, i) => i)));

function toPem(der: ArrayBuffer, label: string): string {
  const b64 = btoa(String.fromCharCode(...new Uint8Array(der)));
  const lines = b64.match(/.{1,64}/g)!.join("\n");
  return `-----BEGIN ${label}-----\n${lines}\n-----END ${label}-----\n`;
}

export async function makeKeys() {
  const ec = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
    "sign",
    "verify",
  ])) as CryptoKeyPair;
  const rsa = (await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"],
  )) as CryptoKeyPair;
  return {
    ecPublic: ec.publicKey,
    rsaPublic: rsa.publicKey,
    apnsP8: toPem(await crypto.subtle.exportKey("pkcs8", ec.privateKey), "PRIVATE KEY"),
    fcmPem: toPem(await crypto.subtle.exportKey("pkcs8", rsa.privateKey), "PRIVATE KEY"),
  };
}

export type Keys = Awaited<ReturnType<typeof makeKeys>>;

export function makeEnv(keys: Keys, overrides: Partial<Env> = {}): Env {
  return {
    RELAY_SECRET,
    APNS_KEY_P8: keys.apnsP8,
    APNS_KEY_ID: "KEYID12345",
    APNS_TEAM_ID: "TEAMID1234",
    APNS_BUNDLE_ID: "com.leeguoo.chromeuse",
    FCM_SERVICE_ACCOUNT_JSON: JSON.stringify({
      type: "service_account",
      project_id: "chrome-use-test",
      client_email: "push@chrome-use-test.iam.gserviceaccount.com",
      private_key: keys.fcmPem,
      token_uri: "https://oauth2.googleapis.com/token",
    }),
    ...overrides,
  };
}

/** Counts calls per key and fails once `max` is exceeded, like the binding. */
export function fakeLimiter(max: number): RateLimiter & { keys: string[] } {
  const counts = new Map<string, number>();
  const keys: string[] = [];
  return {
    keys,
    async limit({ key }) {
      keys.push(key);
      const n = (counts.get(key) ?? 0) + 1;
      counts.set(key, n);
      return { success: n <= max };
    },
  };
}

export function post(path: string, body: unknown, headers: Record<string, string> = {}): Request {
  return new Request(`https://push.example.test${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", "cf-connecting-ip": "203.0.113.7", ...headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

export function decodeJwtPart(part: string): any {
  const b64 = part.replace(/-/g, "+").replace(/_/g, "/");
  return JSON.parse(atob(b64 + "===".slice((b64.length + 3) % 4)));
}

export function jwtSignatureBytes(jwt: string): Uint8Array {
  const sig = jwt.split(".")[2].replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(sig + "===".slice((sig.length + 3) % 4));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}
