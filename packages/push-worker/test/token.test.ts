import { describe, expect, it } from "vitest";
import { base64urlEncode } from "../src/encoding";
import { TOKEN_TTL_SECONDS, signRelayToken, verifyRelayToken } from "../src/token";
import { IOS_TOKEN, RELAY_SECRET } from "./helpers";

const NOW = 1_800_000_000;

describe("relayToken", () => {
  it("round-trips claims and expires after 90 days", async () => {
    const { relayToken, claims } = await signRelayToken(RELAY_SECRET, "ios", IOS_TOKEN, "sandbox", NOW);
    expect(claims).toEqual({ p: "ios", d: IOS_TOKEN, e: "sandbox", iat: NOW, exp: NOW + TOKEN_TTL_SECONDS });
    expect(TOKEN_TTL_SECONDS).toBe(90 * 86400);

    const ok = await verifyRelayToken(RELAY_SECRET, relayToken, NOW + 10);
    expect(ok).toMatchObject({ ok: true, claims });

    expect(await verifyRelayToken(RELAY_SECRET, relayToken, NOW + TOKEN_TTL_SECONDS - 1)).toMatchObject({ ok: true });
    expect(await verifyRelayToken(RELAY_SECRET, relayToken, NOW + TOKEN_TTL_SECONDS)).toEqual({
      ok: false,
      status: "expired_token",
    });
  });

  it("rejects a token signed with another secret", async () => {
    const { relayToken } = await signRelayToken("another-secret-0123456789abcdef0123", "ios", IOS_TOKEN, "production", NOW);
    expect(await verifyRelayToken(RELAY_SECRET, relayToken, NOW)).toEqual({ ok: false, status: "invalid_token" });
  });

  it("rejects tampered claims even when they are well-formed", async () => {
    const { relayToken } = await signRelayToken(RELAY_SECRET, "ios", IOS_TOKEN, "production", NOW);
    const [, sig] = relayToken.split(".");
    const forged = base64urlEncode(
      JSON.stringify({ p: "ios", d: "b2".repeat(32), e: "production", iat: NOW, exp: NOW + TOKEN_TTL_SECONDS }),
    );
    expect(await verifyRelayToken(RELAY_SECRET, `${forged}.${sig}`, NOW)).toEqual({ ok: false, status: "invalid_token" });

    // Extending expiry of an expired token by editing it must not work either.
    const extended = base64urlEncode(
      JSON.stringify({ p: "ios", d: IOS_TOKEN, e: "production", iat: NOW, exp: NOW + 10 * TOKEN_TTL_SECONDS }),
    );
    expect(await verifyRelayToken(RELAY_SECRET, `${extended}.${sig}`, NOW)).toEqual({ ok: false, status: "invalid_token" });
  });

  it("rejects a flipped signature bit", async () => {
    const { relayToken } = await signRelayToken(RELAY_SECRET, "android", "x".repeat(40), "production", NOW);
    const [body, sig] = relayToken.split(".");
    const flipped = (sig[0] === "A" ? "B" : "A") + sig.slice(1);
    expect(await verifyRelayToken(RELAY_SECRET, `${body}.${flipped}`, NOW)).toEqual({ ok: false, status: "invalid_token" });
  });

  it("rejects structurally broken input", async () => {
    for (const bad of [undefined, 42, "", "nodot", "a.b.c", "!!!.???", "a".repeat(9000), "e30.AAAA"]) {
      expect(await verifyRelayToken(RELAY_SECRET, bad, NOW)).toEqual({ ok: false, status: "invalid_token" });
    }
  });

  it("rejects a token issued far in the future", async () => {
    const { relayToken } = await signRelayToken(RELAY_SECRET, "ios", IOS_TOKEN, "production", NOW + 3600);
    expect(await verifyRelayToken(RELAY_SECRET, relayToken, NOW)).toEqual({ ok: false, status: "invalid_token" });
  });
});
