import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { resetApnsCache } from "../src/apns";
import { resetFcmCache } from "../src/fcm";
import worker, { MAX_PAYLOAD_CHARS, MIN_PAYLOAD_BYTES } from "../src/index";
import { signRelayToken } from "../src/token";
import type { Env } from "../src/types";
import {
  FCM_TOKEN,
  IOS_TOKEN,
  PAYLOAD,
  RELAY_SECRET,
  decodeJwtPart,
  fakeLimiter,
  jwtSignatureBytes,
  makeEnv,
  makeKeys,
  post,
  type Keys,
} from "./helpers";

let keys: Keys;
let env: Env;
let fetchMock: ReturnType<typeof vi.fn>;
let logs: string[];

beforeAll(async () => {
  keys = await makeKeys();
});

beforeEach(() => {
  env = makeEnv(keys);
  resetApnsCache();
  resetFcmCache();
  fetchMock = vi.fn();
  vi.stubGlobal("fetch", fetchMock);
  logs = [];
  vi.spyOn(console, "log").mockImplementation((...args) => void logs.push(args.join(" ")));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const call = (req: Request, e: Env = env) => worker.fetch(req, e);

async function register(body: unknown, e: Env = env) {
  const res = await call(post("/v1/register", body), e);
  return { res, body: (await res.json()) as any };
}

async function iosToken(apnsEnv: "sandbox" | "production" = "production") {
  return (await register({ platform: "ios", deviceToken: IOS_TOKEN, env: apnsEnv })).body.relayToken as string;
}

async function androidToken() {
  return (await register({ platform: "android", deviceToken: FCM_TOKEN })).body.relayToken as string;
}

function jsonResponse(status: number, body: unknown) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function fcmOk() {
  fetchMock.mockImplementation(async (url: string) =>
    url.includes("oauth2")
      ? jsonResponse(200, { access_token: "ya29.test", expires_in: 3599, token_type: "Bearer" })
      : jsonResponse(200, { name: "projects/chrome-use-test/messages/1" }),
  );
}

describe("routing", () => {
  it("404s unknown paths and 405s non-POST", async () => {
    expect((await call(new Request("https://x/v1/other", { method: "POST" }))).status).toBe(404);
    const res = await call(new Request("https://x/v1/push", { method: "GET" }));
    expect(res.status).toBe(405);
    expect(res.headers.get("allow")).toBe("POST");
  });

  it("refuses to run without a strong RELAY_SECRET", async () => {
    const { res } = await register({ platform: "ios", deviceToken: IOS_TOKEN }, { ...env, RELAY_SECRET: "short" });
    expect(res.status).toBe(500);
  });
});

describe("POST /v1/register", () => {
  it("issues a token with ISO expiresAt 90 days out", async () => {
    const before = Date.now();
    const { res, body } = await register({ platform: "ios", deviceToken: IOS_TOKEN, env: "sandbox" });
    expect(res.status).toBe(200);
    expect(body.relayToken).toMatch(/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/);
    const exp = Date.parse(body.expiresAt);
    expect(exp - before).toBeGreaterThan(89.9 * 86400_000);
    expect(exp - before).toBeLessThanOrEqual(90 * 86400_000 + 1000);
    expect(decodeJwtPart(body.relayToken.split(".")[0])).toMatchObject({ p: "ios", d: IOS_TOKEN, e: "sandbox" });
  });

  it("validates platform, device token shape and env", async () => {
    for (const bad of [
      { platform: "web", deviceToken: IOS_TOKEN },
      { platform: "ios", deviceToken: "not-hex" },
      { platform: "ios" },
      { platform: "android", deviceToken: "short" },
      { platform: "ios", deviceToken: IOS_TOKEN, env: "staging" },
    ]) {
      expect((await register(bad)).res.status).toBe(400);
    }
    expect((await call(post("/v1/register", "{not json"))).status).toBe(400);
    expect((await call(post("/v1/register", "[]"))).status).toBe(400);
  });

  it("is rate limited per IP", async () => {
    const limiter = fakeLimiter(2);
    const e = { ...env, RL_REGISTER: limiter };
    expect((await register({ platform: "ios", deviceToken: IOS_TOKEN }, e)).res.status).toBe(200);
    expect((await register({ platform: "ios", deviceToken: IOS_TOKEN }, e)).res.status).toBe(200);
    const { res, body } = await register({ platform: "ios", deviceToken: IOS_TOKEN }, e);
    expect(res.status).toBe(429);
    expect(body).toEqual({ status: "rate_limited", retryAfter: 60 });
    expect(limiter.keys).toEqual(["203.0.113.7", "203.0.113.7", "203.0.113.7"]);
  });
});

describe("POST /v1/push validation", () => {
  it("rejects forged and expired tokens with 401 before any upstream call", async () => {
    const forged = await call(post("/v1/push", { relayToken: "e30.AAAA", payload: PAYLOAD, priority: "high" }));
    expect(forged.status).toBe(401);
    expect(await forged.json()).toEqual({ status: "invalid_token" });

    const old = await signRelayToken(RELAY_SECRET, "ios", IOS_TOKEN, "production", Math.floor(Date.now() / 1000) - 91 * 86400);
    const expired = await call(post("/v1/push", { relayToken: old.relayToken, payload: PAYLOAD, priority: "high" }));
    expect(expired.status).toBe(401);
    expect(await expired.json()).toEqual({ status: "expired_token" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("caps payload at 2 KiB of base64 and the body at 16 KiB", async () => {
    const relayToken = await iosToken();
    fetchMock.mockResolvedValue(new Response(null, { status: 200 }));

    const atCap = "A".repeat(MAX_PAYLOAD_CHARS);
    expect((await call(post("/v1/push", { relayToken, payload: atCap, priority: "normal" }))).status).toBe(200);

    const over = "A".repeat(MAX_PAYLOAD_CHARS + 4);
    const res = await call(post("/v1/push", { relayToken, payload: over, priority: "normal" }));
    expect(res.status).toBe(413);

    const huge = await call(post("/v1/push", { relayToken, payload: "A".repeat(20_000), priority: "normal" }));
    expect(huge.status).toBe(413);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("rejects non-base64 payloads, bad priority and bad collapseId", async () => {
    const relayToken = await iosToken();
    for (const body of [
      { relayToken, payload: "not base64 at all!!".repeat(4), priority: "high" },
      { relayToken, payload: "QUJD", priority: "high" },
      // 31 bytes: one short of kid(4) + nonce(12) + tag(16), still 44 chars.
      { relayToken, payload: btoa("x".repeat(31)), priority: "high" },
      // Unpadded base64.
      { relayToken, payload: btoa("x".repeat(40)).replace(/=+$/, ""), priority: "high" },
      { relayToken, payload: PAYLOAD, priority: "urgent" },
      { relayToken, payload: PAYLOAD, priority: "high", collapseId: "x".repeat(65) },
      { relayToken, payload: PAYLOAD, priority: "high", collapseId: "has space" },
    ]) {
      expect((await call(post("/v1/push", body))).status).toBe(400);
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("accepts the smallest valid payload (32 bytes)", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 200 }));
    const relayToken = await iosToken();
    const payload = btoa("x".repeat(MIN_PAYLOAD_BYTES));
    expect(payload).toHaveLength(44);
    expect((await call(post("/v1/push", { relayToken, payload, priority: "high" }))).status).toBe(200);
  });
});

describe("APNs", () => {
  it("sends a generic mutable alert with the opaque payload and a valid ES256 JWT", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 200 }));
    const relayToken = await iosToken("production");
    const res = await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high", collapseId: "conv-1" }));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ status: "sent" });

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(`https://api.push.apple.com/3/device/${IOS_TOKEN}`);
    expect(init.method).toBe("POST");
    const h = init.headers as Record<string, string>;
    expect(h["apns-topic"]).toBe("com.leeguoo.chromeuse");
    expect(h["apns-push-type"]).toBe("alert");
    expect(h["apns-priority"]).toBe("10");
    expect(h["apns-collapse-id"]).toBe("conv-1");
    expect(JSON.parse(init.body)).toEqual({
      aps: {
        alert: { title: "chrome-use", body: "chrome-use needs you" },
        sound: "default",
        "mutable-content": 1,
        "interruption-level": "time-sensitive",
      },
      e: PAYLOAD,
    });

    const jwt = h.authorization.replace(/^bearer /, "");
    const [header, claims] = jwt.split(".");
    expect(decodeJwtPart(header)).toEqual({ alg: "ES256", kid: "KEYID12345" });
    expect(decodeJwtPart(claims)).toMatchObject({ iss: "TEAMID1234" });
    const valid = await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      keys.ecPublic,
      jwtSignatureBytes(jwt),
      new TextEncoder().encode(`${header}.${claims}`),
    );
    expect(valid).toBe(true);
  });

  it("uses the sandbox host, priority 5 and no collapse id for normal sandbox pushes", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 200 }));
    const relayToken = await iosToken("sandbox");
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "normal" }));
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(`https://api.sandbox.push.apple.com/3/device/${IOS_TOKEN}`);
    expect(init.headers["apns-priority"]).toBe("5");
    expect(init.headers["apns-collapse-id"]).toBeUndefined();
    expect(JSON.parse(init.body).aps["interruption-level"]).toBe("active");
  });

  it("reuses the provider JWT across pushes", async () => {
    fetchMock.mockImplementation(async () => new Response(null, { status: 200 }));
    const relayToken = await iosToken();
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    const auths = fetchMock.mock.calls.map(([, init]) => init.headers.authorization);
    expect(auths[0]).toBe(auths[1]);
  });

  it.each([
    [410, { reason: "Unregistered" }, 410, { status: "unregistered", reason: "Unregistered" }],
    [410, {}, 410, { status: "unregistered", reason: "Unregistered" }],
    // Also what a sandbox token sent to production looks like; the reason lets the app tell.
    [400, { reason: "BadDeviceToken" }, 410, { status: "unregistered", reason: "BadDeviceToken" }],
    [400, { reason: "BadCollapseId" }, 502, { status: "upstream_error" }],
    [403, { reason: "ExpiredProviderToken" }, 502, { status: "upstream_error" }],
    [429, { reason: "TooManyRequests" }, 502, { status: "upstream_error" }],
    [500, { reason: "InternalServerError" }, 502, { status: "upstream_error" }],
  ])("maps APNs %i %o to %i", async (upstream, body, expected, responseBody) => {
    fetchMock.mockResolvedValue(jsonResponse(upstream, body));
    const relayToken = await iosToken();
    const res = await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    expect(res.status).toBe(expected);
    expect(await res.json()).toEqual(responseBody);
  });

  it("maps a network failure to 502", async () => {
    fetchMock.mockRejectedValue(new TypeError("network"));
    const relayToken = await iosToken();
    expect((await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }))).status).toBe(502);
  });

  it("drops the cached JWT after InvalidProviderToken", async () => {
    const relayToken = await iosToken();
    fetchMock.mockResolvedValueOnce(jsonResponse(403, { reason: "InvalidProviderToken" }));
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 200 }));
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    // ECDSA signatures are randomized, so a re-minted JWT always differs from the cached one.
    const [first, second] = fetchMock.mock.calls.map(([, init]) => init.headers.authorization);
    expect(first).not.toBe(second);
  });

  it("returns 502 when APNs secrets are missing", async () => {
    const relayToken = await iosToken();
    const res = await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }), {
      ...env,
      APNS_KEY_P8: undefined,
    });
    expect(res.status).toBe(502);
    expect(await res.json()).toEqual({ status: "upstream_error", reason: "misconfigured" });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("FCM", () => {
  it("exchanges an RS256 assertion and sends a data-only message", async () => {
    fcmOk();
    const relayToken = await androidToken();
    const res = await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high", collapseId: "conv-1" }));
    expect(res.status).toBe(200);

    const [tokenUrl, tokenInit] = fetchMock.mock.calls[0];
    expect(tokenUrl).toBe("https://oauth2.googleapis.com/token");
    const form = new URLSearchParams(tokenInit.body);
    expect(form.get("grant_type")).toBe("urn:ietf:params:oauth:grant-type:jwt-bearer");
    const assertion = form.get("assertion")!;
    const [header, claims] = assertion.split(".");
    expect(decodeJwtPart(header)).toEqual({ alg: "RS256", typ: "JWT" });
    expect(decodeJwtPart(claims)).toMatchObject({
      iss: "push@chrome-use-test.iam.gserviceaccount.com",
      scope: "https://www.googleapis.com/auth/firebase.messaging",
      aud: "https://oauth2.googleapis.com/token",
    });
    const valid = await crypto.subtle.verify(
      "RSASSA-PKCS1-v1_5",
      keys.rsaPublic,
      jwtSignatureBytes(assertion),
      new TextEncoder().encode(`${header}.${claims}`),
    );
    expect(valid).toBe(true);

    const [sendUrl, sendInit] = fetchMock.mock.calls[1];
    expect(sendUrl).toBe("https://fcm.googleapis.com/v1/projects/chrome-use-test/messages:send");
    expect(sendInit.headers.authorization).toBe("Bearer ya29.test");
    expect(JSON.parse(sendInit.body)).toEqual({
      message: {
        token: FCM_TOKEN,
        data: { e: PAYLOAD },
        android: { priority: "HIGH", collapse_key: "conv-1" },
      },
    });
  });

  it("caches the access token and uses NORMAL priority", async () => {
    fcmOk();
    const relayToken = await androidToken();
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "normal" }));
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "normal" }));
    const urls = fetchMock.mock.calls.map(([url]) => url as string);
    expect(urls.filter((u) => u.includes("oauth2"))).toHaveLength(1);
    expect(urls.filter((u) => u.includes("messages:send"))).toHaveLength(2);
    expect(JSON.parse(fetchMock.mock.calls[1][1].body).message.android).toEqual({ priority: "NORMAL" });
  });

  const fcmError = (status: number, errStatus: string, errorCode?: string) => ({
    error: {
      code: status,
      status: errStatus,
      details: errorCode
        ? [{ "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError", errorCode }]
        : [],
    },
  });

  it.each([
    [404, fcmError(404, "NOT_FOUND", "UNREGISTERED"), 410, { status: "unregistered", reason: "UNREGISTERED" }],
    [400, fcmError(400, "INVALID_ARGUMENT", "UNREGISTERED"), 410, { status: "unregistered", reason: "UNREGISTERED" }],
    [404, fcmError(404, "NOT_FOUND", "NOT_FOUND"), 410, { status: "unregistered", reason: "NOT_FOUND" }],
    // Project-level 404 (wrong project_id): no FcmError about the token.
    [404, fcmError(404, "NOT_FOUND"), 502, { status: "upstream_error", reason: "misconfigured" }],
    [404, "<html>Not Found</html>", 502, { status: "upstream_error", reason: "misconfigured" }],
    [403, fcmError(403, "PERMISSION_DENIED"), 502, { status: "upstream_error", reason: "misconfigured" }],
    [400, fcmError(400, "INVALID_ARGUMENT", "INVALID_ARGUMENT"), 502, { status: "upstream_error" }],
    [429, fcmError(429, "RESOURCE_EXHAUSTED", "QUOTA_EXCEEDED"), 502, { status: "upstream_error" }],
    [503, fcmError(503, "UNAVAILABLE", "UNAVAILABLE"), 502, { status: "upstream_error" }],
  ])("maps FCM %i %j to %i", async (upstream, body, expected, responseBody) => {
    fetchMock.mockImplementation(async (url: string) =>
      url.includes("oauth2")
        ? jsonResponse(200, { access_token: "ya29.test", expires_in: 3599 })
        : typeof body === "string"
          ? new Response(body, { status: upstream })
          : jsonResponse(upstream, body),
    );
    const relayToken = await androidToken();
    const res = await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    expect(res.status).toBe(expected);
    expect(await res.json()).toEqual(responseBody);
  });

  it("returns 502 when the OAuth exchange fails", async () => {
    fetchMock.mockResolvedValue(jsonResponse(400, { error: "invalid_grant" }));
    const relayToken = await androidToken();
    expect((await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }))).status).toBe(502);
  });
});

describe("rate limits", () => {
  it("limits per device, so freshly minted relayTokens share one budget", async () => {
    fetchMock.mockImplementation(async () => new Response(null, { status: 200 }));
    const limiter = fakeLimiter(2);
    const e = { ...env, RL_TOKEN: limiter };
    const relayToken = await iosToken();
    const send = (token = relayToken) => call(post("/v1/push", { relayToken: token, payload: PAYLOAD, priority: "high" }), e);
    expect((await send()).status).toBe(200);
    // A second relayToken for the same device token (different iat, so a different MAC).
    const now = Math.floor(Date.now() / 1000);
    const minted = (await signRelayToken(RELAY_SECRET, "ios", IOS_TOKEN, "sandbox", now - 5)).relayToken;
    expect(minted).not.toBe(relayToken);
    expect((await send(minted)).status).toBe(200);
    const res = await send((await signRelayToken(RELAY_SECRET, "ios", IOS_TOKEN, "production", now - 9)).relayToken);
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("60");
    expect(await res.json()).toEqual({ status: "rate_limited", retryAfter: 60 });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(new Set(limiter.keys).size).toBe(1);
    expect(limiter.keys[0]).toMatch(/^[0-9a-f]{64}$/);
    expect(limiter.keys.join()).not.toContain(IOS_TOKEN);

    // A different device keeps its own budget.
    fcmOk();
    expect((await send(await androidToken())).status).toBe(200);
    expect(new Set(limiter.keys).size).toBe(2);
  });

  it("limits per source IP before verifying the token", async () => {
    const limiter = fakeLimiter(0);
    const res = await call(post("/v1/push", { relayToken: "x", payload: PAYLOAD, priority: "high" }), {
      ...env,
      RL_IP: limiter,
    });
    expect(res.status).toBe(429);
    expect(limiter.keys).toEqual(["203.0.113.7"]);
  });

  it("lets requests through when the binding is missing or throws", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 200 }));
    const relayToken = await iosToken();
    const broken = { limit: async () => Promise.reject(new Error("binding down")) };
    const res = await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }), {
      ...env,
      RL_IP: broken,
      RL_TOKEN: undefined,
    });
    expect(res.status).toBe(200);
  });
});

describe("logging", () => {
  it("never logs the payload or the full device token", async () => {
    fetchMock.mockResolvedValue(jsonResponse(410, { reason: "Unregistered" }));
    const relayToken = await iosToken();
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    fetchMock.mockResolvedValue(jsonResponse(500, { reason: "InternalServerError" }));
    await call(post("/v1/push", { relayToken, payload: PAYLOAD, priority: "high" }));
    expect(logs.length).toBeGreaterThan(0);
    const all = logs.join("\n");
    expect(all).not.toContain(PAYLOAD);
    expect(all).not.toContain(IOS_TOKEN);
    expect(all).not.toContain(relayToken);
  });
});
