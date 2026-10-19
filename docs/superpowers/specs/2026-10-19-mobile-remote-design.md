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
- Gateway calls the panel service with the profile fixed at pair time, never taken from the phone's request (same property the native host keeps at `agent_panel.rs:583-662`). The profile is the panel profile id (the extension's UUID directory under `agent-panel/profiles/`): `mobile pair --profile <id>`, or the only one present; with several, `pair` lists them and asks. `mobile serve --profile <id>` additionally accepts only devices paired to that profile.
- Phone requests go through `agent_panel::handle_for_profile` (they may start the panel service, as the desktop panel's requests do). Background polling (subscriptions, push watcher) uses `call_running_service` and never starts a service. Panel calls are blocking one-request connections; the gateway runs them on blocking threads with at most 4 in flight.
- Gateway polls `conversationRead` on the phone's behalf for subscribed conversations and pushes events down one multiplexed WebSocket. The phone stores the last `cursor` per conversation, so a reconnect resumes with `after=cursor` and nothing is lost or duplicated.

## Binding and reachability
- `mobile serve --bind <addr>`; default `127.0.0.1:17345` (Cloudflare tunnel runs on the same machine); a bare IP takes port 17345. Binding a non-loopback interface (Tailscale/LAN IP) is explicit opt-in and prints a warning. `0.0.0.0` requires `--allow-all-interfaces`.
- The gateway cannot discover its public URL. `mobile pair --url <reachable-url>` takes it (`ws://` or `wss://`); without `--url` it fails and lists detected Tailscale (100.64.0.0/10) and LAN IPv4 addresses as `ws://<ip>:17345` candidates.
- Behind a tunnel every connection comes from loopback. For a loopback peer only, the gateway takes the client address from `CF-Connecting-IP` for rate limiting and `last IP`.
- Quick tunnels (trycloudflare) are allowed but documented as testing-only: the URL changes per run, which forces re-pairing.

## Pairing
1. `chrome-use mobile pair --url wss://… [--name "Leo's iPhone"]` creates a pending device: 32-byte `pair_secret` (getrandom, as in `native/auth.rs:120-122`), valid 10 min, single use.
2. Terminal shows a QR: `chromeuse://pair?v=1&url=<percent-encoded url>&id=<deviceId>&k=<base64url pair_secret>&fp=<gateway id>`. `deviceId` is a UUID. `fp` is a random per-gateway identity (16 bytes, base64url, stored in `devices.json`; also sent in every challenge as `gateway`). It lets the app tell gateways apart; it is not a key and authenticates nothing (there is no gateway key pair). `pair --json` prints the link instead of a QR.
3. Phone connects, proves possession of `pair_secret` (HMAC challenge, not sending the secret), receives nothing secret back; both sides derive long-term keys with HKDF-SHA256 from `pair_secret`:
   - `auth_key` (per-connection challenge-response),
   - `frame_key` (E2E frame encryption, see below).
4. Pending entry becomes active. The QR is now useless; scanning it again fails: the app's first connection after a scan sends `pairing:true`, which the gateway accepts only for a pending, unexpired device. Two phones racing with one QR: activation is committed under the store lock, so one wins.
   - Caveat (see Open questions): the long-term keys derive from `pair_secret` alone, so whoever holds the link text can still complete a normal (`pairing:false`) handshake after pairing. The link printed in the terminal is a long-term credential.
- Storage: `~/.chrome-use/mobile/devices.json` + `keys/<deviceId>.key` (raw 32 bytes); `$CHROME_USE_RELAY_DIR/mobile` instead when that variable is set, mirroring `agent-panel`. Changes run under `store.lock`, so `pair`, `revoke` and a running gateway do not lose each other's writes. Files: dir 0700, files 0600, via `private_tree` / `atomic_write` (`agent_panel/config_transaction.rs:670, 772`); Windows user-only ACL. Phone: iOS Keychain / Android Keystore.
- All secret comparisons are constant-time (the panel service's `!=` at `service.rs:3548` must not be copied). The gateway compares only the phone proof, with `Mac::verify_slice`.
- `mobile devices [--json]` lists id, name, state (pending/expired/active/disabled), created, last seen, last IP, push on/off. `mobile revoke <id>` deletes the record, key and relay token; a running gateway re-reads the store every 500 ms and closes that device's connections with code 4403.

## Transport and encryption
- Key schedule (HKDF-SHA256, RFC 5869, 32-byte output; `S = "chrome-use mobile v1"`):
  - `auth_key = HKDF(ikm = pair_secret, salt = S, info = "auth")`
  - `frame_key = HKDF(ikm = pair_secret, salt = S, info = "frame")`
  - `push_key = HKDF(ikm = frame_key, salt = empty (32 zero bytes), info = "push")`
  - `c2p_key = HKDF(ikm = frame_key, salt = gatewayNonce‖phoneNonce, info = "c2p")` (gateway → phone), `p2c_key` likewise with `"p2c"` (phone → gateway).
- One WebSocket per phone, any path. Handshake (text frames, JSON; binary values are base64url without padding):
  1. gateway → `{"type":"challenge","v":1,"gatewayNonce":<32 bytes>,"gateway":<fp>}`
  2. phone → `{"type":"auth","v":1,"deviceId","phoneNonce":<32 random bytes>,"proof","pairing":bool}`, `proof = HMAC-SHA256(auth_key, "chrome-use mobile v1 phone proof" ‖ gatewayNonce ‖ phoneNonce ‖ utf8(deviceId))`
  3. gateway → `{"type":"ready","proof"}`, `proof = HMAC-SHA256(auth_key, "chrome-use mobile v1 gateway proof" ‖ gatewayNonce ‖ phoneNonce ‖ utf8(deviceId))`. The phone must verify it before sending anything encrypted.
  - Changed from the first draft (`HMAC(auth_key, nonce‖deviceId)`): the proof binds the phone's nonce too (it salts the session keys), and the two directions use different labels so neither proof can be reflected.
  - Refusal is a close frame only, never a reason: 4401 (unknown/wrong key/wrong mode/revoked/disabled), 4429 (backoff), 4408 (no reply within 10 s), 4400 (malformed). After the handshake: 4400 for a rejected frame, 4403 when the device is revoked or disabled.
- Every frame after the handshake: AES-256-GCM (available natively in CryptoKit, Android `javax.crypto` and the existing `aes-gcm` crate). Per-connection keys `HKDF(frame_key, salt = gatewayNonce‖phoneNonce, info = "c2p"|"p2c")`, one per direction; nonce = 4 zero bytes ‖ u64 BE sequence number, starting at 0 and strictly increasing, so nonces never repeat under one key. Each WebSocket binary message is `seq (8 bytes, BE) ‖ ciphertext ‖ tag (16)`, with the same 8 sequence bytes as associated data. A frame whose sequence is not exactly the next expected one, or that fails authentication, closes the connection (4400). Text frames after the handshake also close it. Plaintext is UTF-8 JSON. Messages are capped at 2 MiB.
- Push payloads use `push_key` with a random 12-byte nonce carried in the payload, no associated data (pushes are not on a connection).
- Rationale: Cloudflare tunnels terminate TLS at the edge, so TLS alone would let the tunnel provider read page content and instructions. Both ends are the user's devices, so a symmetric key carried in the QR is sufficient; no PKI.
- Rate limit: failed handshakes per source address and per claimed device id. The first 2 failures are free; from the 3rd, the source (or device) waits 1 s, doubling per failure up to 300 s; a success clears both. A blocked source is dropped before the WebSocket upgrade or closed with 4429. Wrong proofs for a known device are also counted persistently (`failures` in `devices.json`); 10 consecutive ones set it `disabled` until it is paired again. At most 32 connections at once.

## Phone protocol (inside encrypted frames)
`{id, op, params}` → `{id, result}` | `{id, error:{code, message}}`; `id` is echoed as sent. Error codes: `denied` (op or field not allowed from the phone; never forwarded), `invalidParams`, `notFound`, `limit`, `rateLimited`, `panel` (the panel service refused; `message` is its text), `screenshot`, `store`. Up to 8 requests run concurrently per connection; more wait.

Server events:
- `{event:"journal", conversationId, conversation, events, cursor, more, queuedMessages}`: the `conversationRead` result for a subscription, sent when there are new events or the conversation summary changed. `cursor` is the value to resume from.
- `{event:"subscriptionError", conversationId, error}` once per run of failed polls (the service is not running, …); polling continues every 3 s.
- `{event:"subscriptionEnded", conversationId, reason}` when the conversation no longer exists.

Allowed ops (mapped 1:1 to panel operations unless noted):
- `discover`, `conversationList`, `scheduleList`: `{}` only.
- `conversationCreate {agentId, model?, options?, fullAccess?}`: `fullAccess` may only be `false` and is always sent as `false`. The effective tool access (`toolAccess`, else the agent default: OpenCode → runCommands, Codex → readFiles, others → browserOnly) must be at most `writeFiles`, so `runCommands` and OpenCode conversations are created on the desktop only.
- `conversationSend` / `conversationSendDirect {conversationId, messageId, text, attachments?, record?}`: `record` only `false` (dropped); at most 5 attachments, each `bytes` ≤ 512 KiB (the service re-verifies the files).
- `conversationStop {conversationId}`, `conversationRead {conversationId, after?}` (initial page).
- `conversationRespond {conversationId, requestId, generation, requestRevision, decision}`: all five required; the legacy `result` form is refused.
- `conversationAttachmentPut {conversationId, data}`: base64 length re-checked against 512 KiB before forwarding.
- `conversationDelete {conversationId, confirm:true}`: forwarded as `{conversationId}`.
- `conversationSetOptions {conversationId, options:{effort?, serviceTier?, toolAccess?}}`: refused if the new effective tool access is higher than the conversation's current one (read from `conversationList`).
- `scheduleRun {id}`; `scheduleSave {id, enabled}` exactly, for a schedule that already exists (checked with `scheduleList`, since `scheduleSave` with an unknown id would create one). A refusal from the service (for example an unconfirmed previous run that must be acknowledged on the desktop) is passed through.
- Conversation and schedule ids must be UUIDs.
- `subscribe {conversationId, after?}` / `unsubscribe {conversationId}` — gateway-side polling of `conversationRead` every 600 ms while the conversation is busy (starting/running/waitingInput/stopping), 1800 ms otherwise, immediately again while `more` is true. At most 16 per connection; subscribing again to the same conversation restarts it from the new `after`. Subscriptions end with the connection.
- `pushRegister {relayToken}` / `pushUnregister {}` — stores the push relay token for this device (see Notifications). The app should send `pushRegister` on every connection, since the gateway drops tokens the Worker rejects.
- `screenshot {conversationId}` — the conversation's browser session is `panel-<conversationId>` (the panel service's own name for it: `summary().target.session`, used by `close_browser_session` / `adopt_granted_tab`). The conversation must be in this device's profile. The gateway runs `chrome-use screenshot <tmp>.png` with `AGENT_BROWSER_BROWSER=<profile>` and `AGENT_BROWSER_SESSION=panel-<id>`, only if that session's daemon is already running (`connection::daemon_ready`): a phone never starts a daemon or opens a browser. Result `{mediaType:"image/jpeg", width, height, data:<base64>}`, at most 1080 px wide, JPEG quality 70; ≤1 per 2 s per conversation (`rateLimited`); no running session → error `screenshot`.

Denied from the phone in v1:
- `conversationSetAccess` / any `fullAccess=true`, `conversationProjectChoose` (opens a native dialog on the desktop), `serviceStop`, skill writes/imports, schedule create/edit beyond `enabled`, `config*`, recovery/evidence ops, `conversationResume` of sessions not created from the panel.
- Added during implementation: `conversationCreate.executable` (an arbitrary program path) and `.project` (an arbitrary folder); `conversationSend.page`, `.tabGrant` (lends a desktop tab), `.skillId` (needs `conversationSkillSelect`, not allowed), `record:true`; `conversationRespond.result`; `conversationAttachmentRead`, `conversationArtifactRead`, queue edits, `conversationTabGrant`, `conversationSkillSelect`, `conversationSessionList`, `conversationProjectList`, `health`/`serviceHealth`, `agent*` tool operations, inspect operations, `imageConvert`; `scheduleDelete`; raising tool access through `conversationSetOptions`.
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
- Content is end-to-end encrypted: the gateway encrypts `{v, conversationId, kind, title, body}` with `push_key` (derived from `frame_key`); the Worker forwards the opaque blob.
  - iOS: `mutable-content: 1` alert with a generic fallback ("chrome-use needs you"); a Notification Service Extension decrypts and rewrites title/body. The key is shared with the extension via an App Group keychain entry.
  - Android: FCM data message; the app decrypts and posts the notification itself.
- Triggers: `waitingInput` (high priority, time-sensitive on iOS), turn end (`terminal`, normal priority), scheduled run finished/failed. Per-conversation mute on the phone; the gateway debounces to ≤1 push per conversation per 10 s.
- Gateway watcher (implemented): every 2 s, for each profile that has an active device with a relay token, it polls `conversationList` and `scheduleList` of the running service (never starting one). The first poll of a profile is a baseline and announces nothing. Then:
  - a conversation entering `waitingInput` (from any other phase, or new) → `needsInput`, `high`, title = conversation title (first line, ≤80 chars), body `"Waiting for your approval"` (the request itself is not read for the push);
  - a conversation leaving a busy phase for a non-busy one, without `scheduleId` → `turnEnded`, `normal`, body = `lastTurn.preview` (already redacted, ≤120 chars) or Finished/Stopped/Failed;
  - a schedule whose `finishedAt` changed → `scheduleDone` (`lastResult == "completed"`) or `scheduleFailed` (body = `lastResult`, ≤120 chars), conversationId = `lastChat`, title = schedule name.
  - Debounce key is the conversation id (the schedule name when there is none). A notice inside the 10 s window waits and replaces the waiting one, except that a waiting `needsInput` is not replaced by a lesser kind; it goes out when the window closes.
  - `collapseId` = conversation id. Worker replies: 410, and 401 `invalid_token`/`expired_token`, drop that token from the device (the app registers again on its next connection); 429 pauses pushes to that device for `retryAfter` seconds; other failures are logged.
  - Worker URL: `--push-url`, else `CHROME_USE_PUSH_URL`, else `https://push.chrome-use.leeguoo.com`.
- `chrome-use mobile push-test <deviceId>` sends `kind:"test"` (title "chrome-use", body "Test notification (<unix time>)", empty conversationId, no collapseId). Apps should treat unknown kinds like `test`: show title/body, open the app.
- Summary text is capped (≤120 chars) and comes from the agent's final text or the redacted request; even encrypted, page content and tool output are not included.
- Abuse limits in the Worker: per-minute burst caps via the Workers Rate Limiting binding: per `relayToken` (10/60 s), per source IP on push (120/60 s) and on register (10/60 s); `relayToken` expires after 90 days and the app silently refreshes it. APNs `410 Unregistered` / FCM `UNREGISTERED` is returned to the gateway, which drops the token.
- Revocation: `mobile revoke` deletes the device's `relayToken` on the gateway; the Worker never needs to know.
- Cost: Workers free tier (100k requests/day) covers early usage; no KV/D1/DO needed. Track with the cloudflare-cost-aware rules if a store is ever added.
- Worker API (JSON over HTTPS):
  - `POST /v1/register {platform: "ios"|"android", deviceToken, env?: "sandbox"|"production"}` → `{relayToken, expiresAt}`. `relayToken = base64url(JSON{p,d,e,iat,exp}) "." base64url(HMAC-SHA256(RELAY_SECRET, first part))`.
  - `POST /v1/push {relayToken, payload, priority: "high"|"normal", collapseId?}` → `200 {status:"sent"}` | `401 {status:"invalid_token"|"expired_token"}` | `410 {status:"unregistered"}` | `429 {status:"rate_limited", retryAfter}` | `502 {status:"upstream_error"}`.
  - `payload` = base64(nonce(12) ‖ AES-256-GCM ciphertext+tag) of UTF-8 JSON `{v:1, conversationId, kind:"needsInput"|"turnEnded"|"scheduleDone"|"scheduleFailed"|"test", title, body}` under the push key; ≤ 2 KiB. The Worker never parses it.
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
- Implementation: `cli/src/mobile/` (`crypto`, `store`, `policy`, `gateway`, `push`, `panel`); integration test `cli/tests/mobile_gateway.rs` drives a spec-written phone against a fake panel service and a fake push Worker.

## Open questions (from the gateway implementation)
- QR as long-term credential: keys derive from `pair_secret` alone, and the link is printed in the terminal (scrollback, logs). Proposal: on the pairing connection the phone sends a fresh 32-byte `phoneSecret` in its first encrypted frame and both sides switch to `root = HKDF(pair_secret, salt = phoneSecret, info = "root")`, so a leaked link stops working once pairing completes (unless the attacker also captured the pairing traffic). Not implemented; needs the apps to agree.
- The panel service caches one journal read offset per conversation; the gateway and the desktop panel polling the same conversation with different cursors defeat the cache and make each read rescan from the start of `events.jsonl`. Fine for normal journals; worth a per-reader cache if large ones get slow.
- `needsInput` push body is generic. A better line (tool name, Codex command summary) needs a journal read of the request event and a decision on how much of a redacted request may travel even encrypted.
- OpenCode (always runCommands) cannot be created from the phone under the writeFiles cap; confirm this is the wanted rule.
- The integration test is Unix-only (Windows would need the ACL fixture used by `agent_panel_native.rs`).

## Appendix: test vectors
All values hex unless noted. Inputs: `pair_secret = 00 01 … 1f`, `gatewayNonce = 20 21 … 3f`, `phoneNonce = 40 41 … 5f`, `deviceId = "0f8fad5b-d9cb-469f-a165-70867728950e"`. Checked by `mobile::crypto::tests::spec_test_vectors` and computed independently with Python `cryptography`.

| Value | Result |
|---|---|
| `auth_key` | `a8344e0caf1c7d238af084ccdbeb9d4c2d8ac1abeea4314ca22d1d3aaa47e00e` |
| `frame_key` | `1ab8cb618f09a935128d1aca5e0aa708610fd20cb23a3b473e2446052036400d` |
| `push_key` | `27654b22292a6eab25b4c62370b206e71aac17dfef02e6a2cbf568565f85cd2e` |
| `c2p_key` | `5bc1cde1dbe85a0bafb5c37868e4b8798a5d290b548b2c8eea7831449f33116a` |
| `p2c_key` | `5f2d6790a8083d40200190969fa564fae1390dacab76bd4318c41185f10a6845` |
| phone proof | `8dc9b0be57af6d6329053ba1807835eb333d617132c1f1c00d494fa21b99ec20` (base64url `jcmwvlevbWMpBTuhgHg16zM9YXEywfHADUlPohuZ7CA`) |
| gateway proof | `7fcae67185c35186b70f2434a8ba5d593d40b2ea074558601cdd76b1e43f34e6` |

First phone frame (p2c, sequence 0), plaintext `{"id":1,"op":"conversationList","params":{}}`; the WebSocket binary message is:

```
0000000000000000eecbbe0b87d98c8799928139b13079008dd2677e13613735094e30ac54db12d587633832f3eda22f16275d8d6cc4cb8d1b21400427884809662cfab2
```

Push payload with nonce `60 61 … 6b` and plaintext
`{"v":1,"conversationId":"7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e","kind":"needsInput","title":"Weekly report","body":"Waiting for your approval"}` (standard base64):

```
YGFiY2RlZmdoaWprqoPM7QCXZkhyaRefvHAqHgEY+UOzCuAGhcMb8AMnhTeophCgMirT+P2+EAJ4WLnl48e4jehHaMj98HzDBDhx1zqQUyMs71Fy46iVJiGOKMmmDYeTkc51WH7yIMyUZfp4CADxrSXLZ8BPuy/0RvOBhHcvWPqF/CEQM3EYZ0eNtuz3yRqkiOABYIXUrTn31OrXOL/ukGxSZoSNvQCn4gM=
```
