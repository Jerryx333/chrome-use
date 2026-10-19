# chrome-use push relay Worker

Stateless Cloudflare Worker that forwards end-to-end encrypted notifications from a
`chrome-use mobile serve` gateway to APNs (iOS) and FCM (Android). It holds the APNs key
and the Firebase service account and nothing else: no KV, D1, Durable Objects or logs of
content. Design: `docs/superpowers/specs/2026-10-19-mobile-remote-design.md`, section
"Notifications".

## API

| Request | Response |
| --- | --- |
| `POST /v1/register {platform: "ios"\|"android", deviceToken, env?: "sandbox"\|"production"}` | `200 {relayToken, expiresAt}` (ISO 8601, 90 days) |
| `POST /v1/push {relayToken, payload, priority: "high"\|"normal", collapseId?}` | `200 {status:"sent"}`, `401 {status:"invalid_token"\|"expired_token"}`, `410 {status:"unregistered"}`, `429 {status:"rate_limited", retryAfter}`, `502 {status:"upstream_error"}` |

Also: `400 {status:"bad_request", reason}` for malformed input, `413 {status:"too_large"}`
when `payload` exceeds 2048 base64 characters or the body exceeds 16 KiB, `404`/`405` for
other paths and methods, `500 {status:"misconfigured"}` when `RELAY_SECRET` is missing.

- `relayToken = base64url(JSON{p,d,e,iat,exp}) "." base64url(HMAC-SHA256(RELAY_SECRET, first part))`.
  The Worker verifies it on every push; nothing is stored. Rotating `RELAY_SECRET`
  invalidates every token (apps re-register silently on `401`).
- `payload` is opaque base64 ciphertext. The Worker checks only its charset and length;
  it never decodes it. iOS receives it as custom key `e` next to a generic alert
  ("chrome-use needs you", `mutable-content: 1`); Android receives a data-only message `{e}`.
- `env` only matters for iOS: development builds get `sandbox` tokens and must register
  with `env: "sandbox"`.

## Rate limits

Workers Rate Limiting bindings (`wrangler.toml`), no storage:

| Binding | Key | Limit |
| --- | --- | --- |
| `RL_TOKEN` | relayToken MAC segment | 10 / 60 s |
| `RL_IP` | `CF-Connecting-IP` on `/v1/push` | 120 / 60 s |
| `RL_REGISTER` | `CF-Connecting-IP` on `/v1/register` | 10 / 60 s |

The binding only supports 10 s or 60 s windows, so the spec's "about 60 per hour per
token" is enforced as a per-minute burst cap; the gateway's own debounce (≤1 push per
conversation per 10 s) keeps normal use far below it. Counters are per Cloudflare
location and approximate. If the binding is missing or errors, requests are allowed.

## Secrets

| Secret | Value |
| --- | --- |
| `RELAY_SECRET` | ≥32 random characters, e.g. `openssl rand -base64 32` |
| `APNS_KEY_P8` | Contents of `AuthKey_<KEYID>.p8` (PKCS#8 PEM) |
| `APNS_KEY_ID` | The 10-character key id |
| `APNS_TEAM_ID` | Apple Developer team id |
| `APNS_BUNDLE_ID` | The iOS app bundle id (APNs topic) |
| `FCM_SERVICE_ACCOUNT_JSON` | Firebase service account JSON file contents |

```sh
cd packages/push-worker
openssl rand -base64 32 | npx wrangler@4 secret put RELAY_SECRET
npx wrangler@4 secret put APNS_KEY_P8 < AuthKey_ABCDE12345.p8
npx wrangler@4 secret put APNS_KEY_ID        # paste ABCDE12345
npx wrangler@4 secret put APNS_TEAM_ID
npx wrangler@4 secret put APNS_BUNDLE_ID
npx wrangler@4 secret put FCM_SERVICE_ACCOUNT_JSON < service-account.json
```

### Getting the APNs key

Apple Developer → Certificates, Identifiers & Profiles → Keys → **+**, enable
"Apple Push Notifications service (APNs)", download `AuthKey_<KEYID>.p8`. It can be
downloaded only once; keep it in the password manager. The key id is shown on the key
page, the team id under Membership. One token-based key serves both sandbox and production
and every app in the team.

### Getting the Firebase service account

Firebase console → the project → Project settings → Service accounts → "Generate new
private key". The JSON's `project_id` selects the FCM project. The account needs the
"Firebase Cloud Messaging API Admin" role (granted by default to the Admin SDK account),
and the "Firebase Cloud Messaging API (V1)" must be enabled in Google Cloud.

## Develop, test, deploy

```sh
npm install
npm test            # vitest, upstreams mocked
npm run typecheck
npx wrangler@4 deploy --dry-run --outdir /tmp/push-worker-build   # bundle check
npm run deploy      # after the secrets are set
```

The custom domain is still open in the spec; until it is decided the Worker is served on
`chrome-use-push.<account>.workers.dev`. Uncomment `routes` in `wrangler.toml` when the
hostname is fixed.

## Cost

Workers free tier: 100k requests/day, 10 ms CPU per request. A push costs one request
plus one APNs/FCM subrequest; the JWT and OAuth token are cached per isolate (APNs JWT
50 min, Google access token until 5 min before expiry), so signing is rare. Rate-limit
binding calls are free and write nothing. There are no per-request storage writes, so the
only scaling cost is requests ($0.30/M on the paid plan). If a store is ever added, review
it with the cloudflare-cost-aware rules first.

## Logging

Only upstream failures are logged: platform, the first 6 characters and length of the
device token, and the upstream status/reason. Payloads, full device tokens and relay
tokens are never logged.
