# Mobile remote design (requirements v1)

Status: requirements draft. Not an implementation authorization. Open items are marked OPEN.

## Goal
Let the user drive the desktop agent panel from a phone while away from the computer: start a turn, watch it progress, answer the agent's approval prompts, receive results. The phone is a remote for the desktop; the agent and the logged-in Chrome stay on the desktop.

## Confirmed by user
1. iOS + Android apps, one-time unlock at USD 1.99, 14-day free trial.
2. No server operated by us for traffic. Connectivity is bring-your-own: the user's Tailscale, the user's Cloudflare tunnel, or the same LAN.
4. We run push notifications ourselves, as a stateless Cloudflare Worker (no own servers).
3. A hosted relay (Cloudflare Worker + Durable Object) may come later as an optional zero-config mode; this spec must not need it, and its protocol must not change when it arrives.

## Scope v1
- New desktop process `chrome-use mobile serve` (the gateway) plus `chrome-use mobile pair|devices|revoke`.
- Phone features: conversation list, create, send (text + images), live progress, stop, approval answers, screenshot on demand, schedule list/run/enable.
- Out of scope v1: remote clicking/typing on the page, captcha/2FA solving from the phone, the hosted relay, voice, iphone-use bridging.
- OPEN: compare against 云谦's project once the user shares it.

## Existing surfaces (facts this design builds on)
- Panel service: TCP on `127.0.0.1:<ephemeral>`, frame = u32 LE length + JSON (max 768 KiB), one request per connection, ≤16 concurrent, 10 s request read, 64-hex `capability` in `service.json` (`cli/src/agent_panel/service.rs:47-64, 204-207, 3516-3553`). Client: `service::call` (`service.rs:175-231`), which also starts/reuses the service.
- No push: the panel polls `conversationRead {conversationId, after}` → `{conversation, journal:{events,cursor,more}, queuedMessages}` every 600 ms busy / 1800 ms idle (`extensions/ab-connect/sidepanel.js:1157-1194`).
- Approvals: backend `request` event → phase `waitingInput` → `conversationRespond {conversationId, requestId, generation, requestRevision, decision}` (`service.rs:1701-1767, 2919-2985`, decisions in `agent_panel/approval.rs:41-112`).
- Constraints the phone UI must respect: model is set only at create/resume; `conversationSetOptions` and delete are refused while busy; ≤5 images × 512 KiB per message (`attachments.rs:11-20`).
- Stream server (`cli/src/native/stream/`) has no token; it relies on loopback Host/Origin checks, which a tunnel defeats.
- No link exists today between the panel and captcha/2FA handoff (`session handoff`, `.owner` file, `human_check_verdict`).

## Architecture
```
phone app ──WSS/WS──▶ [user's tunnel / tailnet / LAN] ──▶ chrome-use mobile serve ──service::call──▶ panel service
                                                                   └──daemon command (screenshot)──▶ session daemon
```
- Gateway is the only thing exposed. It never proxies `/api/v1/*`, the stream WebSocket, the panel TCP port, or the panel `capability`.
- Gateway calls the panel service with the profile fixed at pair time, never taken from the phone's request (same property the native host keeps at `agent_panel.rs:583-662`).
- Gateway polls `conversationRead` on the phone's behalf for subscribed conversations and pushes events down one multiplexed WebSocket. The phone stores the last `cursor` per conversation, so a reconnect resumes with `after=cursor` and nothing is lost or duplicated.

## Binding and reachability
- `mobile serve --bind <addr>`; default `127.0.0.1:<port>` (Cloudflare tunnel runs on the same machine). Binding a non-loopback interface (Tailscale/LAN IP) is explicit opt-in and prints a warning. `0.0.0.0` requires `--allow-all-interfaces`.
- The gateway cannot discover its public URL. `mobile pair --url <reachable-url>` takes it; without `--url` it offers detected Tailscale/LAN addresses.
- Quick tunnels (trycloudflare) are allowed but documented as testing-only: the URL changes per run, which forces re-pairing.

## Pairing
1. `chrome-use mobile pair --url wss://… [--name "Leo's iPhone"]` creates a pending device: 32-byte `pair_secret` (getrandom, as in `native/auth.rs:120-122`), valid 10 min, single use.
2. Terminal shows a QR: `chromeuse://pair?v=1&url=…&id=<deviceId>&k=<base64url pair_secret>&fp=<gateway key fingerprint>`.
3. Phone connects, proves possession of `pair_secret` (HMAC challenge, not sending the secret), receives nothing secret back; both sides derive long-term keys with HKDF-SHA256 from `pair_secret`:
   - `auth_key` (per-connection challenge-response),
   - `frame_key` (E2E frame encryption, see below).
4. Pending entry becomes active. The QR is now useless; scanning it again fails.
- Storage: `~/.chrome-use/mobile/devices.json` + per-device key files, dir 0700, files 0600, via `private_tree` / `atomic_write` (`agent_panel/config_transaction.rs:670, 772`); Windows user-only ACL. Phone: iOS Keychain / Android Keystore.
- All secret comparisons are constant-time (the panel service's `!=` at `service.rs:3548` must not be copied).
- `mobile devices` lists name, created, last seen, last IP. `mobile revoke <id>` deletes keys and drops live connections immediately.

## Transport and encryption
- One WebSocket per phone. Handshake: gateway nonce → phone `HMAC(auth_key, nonce‖deviceId)` → gateway verifies, replies with its own proof (mutual).
- Every frame after the handshake: AES-256-GCM (available natively in CryptoKit, Android `javax.crypto` and the existing `aes-gcm` crate). Per-connection keys `HKDF(frame_key, salt = gatewayNonce‖phoneNonce, info = "c2p"|"p2c")`, one per direction; nonce = 4 zero bytes ‖ u64 BE sequence number, starting at 0 and strictly increasing, so nonces never repeat under one key. A frame whose sequence is not exactly the next expected one closes the connection.
- Push payloads use a separate key `HKDF(frame_key, info = "push")` with a random 12-byte nonce carried in the payload (pushes are not on a connection).
- Rationale: Cloudflare tunnels terminate TLS at the edge, so TLS alone would let the tunnel provider read page content and instructions. Both ends are the user's devices, so a symmetric key carried in the QR is sufficient; no PKI.
- Rate limit: failed handshakes per IP and per device, with backoff; 10 consecutive failures for a device disable it until re-paired.

## Phone protocol (inside encrypted frames)
`{id, op, params}` → `{id, result|error}`; server events `{event, conversationId, ...}`.

Allowed ops (mapped 1:1 to panel operations unless noted):
- `discover`, `conversationList`, `conversationCreate` (`fullAccess` forced false), `conversationSend`, `conversationSendDirect`, `conversationStop`, `conversationRespond`, `conversationRead` (initial page), `conversationAttachmentPut` (gateway re-checks size/count), `conversationDelete` (phone must send `confirm:true`), `conversationSetOptions`, `scheduleList`, `scheduleRun`, `scheduleSave` limited to toggling `enabled`.
- `subscribe {conversationId, after}` / `unsubscribe` — gateway-side polling and push.
- `pushRegister {relayToken}` / `pushUnregister` — stores the push relay token for this device (see Notifications).
- `screenshot {conversationId}` — gateway resolves the conversation's browser session and runs the daemon screenshot action; JPEG, downscaled for mobile, ≤1 per 2 s per conversation.

Denied from the phone in v1:
- `conversationSetAccess` / any `fullAccess=true`, `conversationProjectChoose` (opens a native dialog on the desktop), `serviceStop`, skill writes/imports, schedule create/edit beyond `enabled`, `config*`, recovery/evidence ops, `conversationResume` of sessions not created from the panel.
- Any op not on the allowlist is rejected by the gateway, not forwarded.

## "Needs you" path
- v1 source: `request` events / phase `waitingInput`. The phone renders the redacted request and offers exactly the decision values valid for that backend method (`approval.rs:41-112`), echoing `generation` and `requestRevision`.
- Captcha/2FA: not wired in v1 (no panel event exists). Later phase: emit a panel event from `human_check_verdict` / login-wall detection, and let the phone answer via screenshot + tap using the stream server's `input_touch` behind the gateway.

## Notifications
Push goes through our Cloudflare Worker `push.chrome-use.leeguoo.com` (OPEN: final hostname), which holds the APNs key and FCM service account and nothing else.

```
gateway ──POST /v1/push {relayToken, ciphertext}──▶ push Worker ──▶ APNs / FCM ──▶ phone (decrypts locally)
```

- Registration: the app gets its APNs/FCM device token, calls `POST /v1/register {platform, deviceToken}` on the Worker, and receives `relayToken = MAC(worker_secret, platform‖deviceToken‖issuedAt)`. The Worker stores nothing; the token is self-verifying. The app sends `relayToken` to the gateway over the encrypted channel (`pushRegister` op); the gateway stores it with the device record.
- Content is end-to-end encrypted: the gateway encrypts `{conversationId, kind, title, summary}` with `frame_key`; the Worker forwards the opaque blob.
  - iOS: `mutable-content: 1` alert with a generic fallback ("chrome-use needs you"); a Notification Service Extension decrypts and rewrites title/body. The key is shared with the extension via an App Group keychain entry.
  - Android: FCM data message; the app decrypts and posts the notification itself.
- Triggers: `waitingInput` (high priority, time-sensitive on iOS), turn end (`terminal`, normal priority), scheduled run finished/failed. Per-conversation mute on the phone; the gateway debounces to ≤1 push per conversation per 10 s.
- Summary text is capped (≤120 chars) and comes from the agent's final text or the redacted request; even encrypted, page content and tool output are not included.
- Abuse limits in the Worker: per-minute burst caps via the Workers Rate Limiting binding: per `relayToken` (10/60 s), per source IP on push (120/60 s) and on register (10/60 s); `relayToken` expires after 90 days and the app silently refreshes it. APNs `410 Unregistered` / FCM `UNREGISTERED` is returned to the gateway, which drops the token.
- Revocation: `mobile revoke` deletes the device's `relayToken` on the gateway; the Worker never needs to know.
- Cost: Workers free tier (100k requests/day) covers early usage; no KV/D1/DO needed. Track with the cloudflare-cost-aware rules if a store is ever added.
- Worker API (JSON over HTTPS):
  - `POST /v1/register {platform: "ios"|"android", deviceToken, env?: "sandbox"|"production"}` → `{relayToken, expiresAt}`. `relayToken = base64url(JSON{p,d,e,iat,exp}) "." base64url(HMAC-SHA256(RELAY_SECRET, first part))`.
  - `POST /v1/push {relayToken, payload, priority: "high"|"normal", collapseId?}` → `200 {status:"sent"}` | `401 {status:"invalid_token"|"expired_token"}` | `410 {status:"unregistered"}` | `429 {status:"rate_limited", retryAfter}` | `502 {status:"upstream_error"}`.
  - `payload` = base64(nonce(12) ‖ AES-256-GCM ciphertext+tag) of UTF-8 JSON `{v:1, conversationId, kind:"needsInput"|"turnEnded"|"scheduleDone"|"scheduleFailed", title, body}` under the push key; ≤ 2 KiB. The Worker never parses it.
  - APNs: alert `{title:"chrome-use", body:"chrome-use needs you"}` (generic fallback), `mutable-content:1`, custom key `e` = payload, `apns-priority` 10/5, `interruption-level: time-sensitive` for `high`. FCM HTTP v1: data-only message `{e: payload}`, `android.priority` HIGH/NORMAL.
- Fallback when push is unavailable (no Google Play services, user denied permission): optional Bark/ntfy webhook with the same generic, content-free body.

## Unlock and trial
- Entirely in the app. iOS: StoreKit 2 non-consumable unlock + USD 0 non-consumable "trial" IAP, verified on-device. Android: Play one-time product; 14-day trial tracked locally (resettable by reinstall; accepted at this price).
- The gateway is open-source and does not check purchase state.

## Trust model
- The phone is trusted as the paired user, but gets less than the desktop: no full access, no config changes, no native dialogs.
- Everything coming back from the agent or the page is untrusted display data on the phone; links in it are not auto-opened.
- A stolen phone is handled by `mobile revoke`; the app also supports an optional biometric lock.
- The gateway never raises an existing conversation's permissions.

## Acceptance
- Pair via QR over Tailscale and over a named Cloudflare tunnel; re-scanning the same QR fails; revoke drops the live connection within 1 s.
- Captured tunnel traffic contains no plaintext instructions, page text or screenshots.
- Wrong / replayed / reordered frames are rejected; brute-force handshakes are rate-limited.
- From the phone: create, send with 2 images, watch progress, answer a Claude Code `can_use_tool` and a Codex `commandExecution` approval, stop a turn, take a screenshot. Desktop panel shows the same conversation consistently.
- Network drop mid-turn → reconnect resumes from the stored cursor with no missing or duplicated events.
- Every denied op returns a clear error and never reaches the panel service.
- Agent asks for approval while the app is killed → iOS and Android show a decrypted "needs you" notification within 5 s; tapping it opens that conversation's request. Worker logs contain no plaintext. A forged or expired `relayToken` is rejected.
- `/api/v1/*` and the stream WebSocket remain unreachable through the tunnel.

## Notes
- Sibling: the pairing + encrypted WebSocket layer should be reusable by iphone-use; keep it in its own module, but don't design for iphone-use in v1.
