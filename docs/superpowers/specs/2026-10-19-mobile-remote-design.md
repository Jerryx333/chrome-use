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
- Behind a tunnel every connection comes from loopback. Only with `mobile serve --trust-cf-header` (meant for cloudflared on the same computer) does the gateway take a loopback peer's client address from `CF-Connecting-IP`, for rate limiting and `last IP`. In that mode the loopback address itself is never checked or struck: source checks run after the upgrade, on the header's address; a loopback connection without the header has no rate-limit source. Without the flag the header is ignored.
- WebSocket upgrades that carry an `Origin` header are refused with HTTP 403: browsers always send one and the apps never do, so no web page (on this computer or elsewhere) can open the gateway.
- Quick tunnels (trycloudflare) are allowed but documented as testing-only: the URL changes per run, which forces re-pairing.

## Pairing
1. `chrome-use mobile pair --url wss://… [--name "Leo's iPhone"]` creates a pending device: 32-byte `pair_secret` (getrandom, as in `native/auth.rs:120-122`), valid 10 min, single use.
2. Terminal shows a QR: `chromeuse://pair?v=1&url=<percent-encoded url>&id=<deviceId>&k=<base64url pair_secret>&fp=<gateway id>`. `n` is the computer's display name (percent-encoded): `mobile serve --name` (saved in `devices.json`), else the macOS computer name (`scutil --get ComputerName`), else the host name; the app shows it instead of the URL host. `deviceId` is a UUID. `fp` is a random per-gateway identity (16 bytes, base64url, stored in `devices.json` on first use; also sent in every challenge as `gateway`). It lets the app tell gateways apart; it is not a key and authenticates nothing (there is no gateway key pair). `pair --json` prints the link instead of a QR.
3. Phone connects, proves possession of `pair_secret` (HMAC challenge, not sending the secret), receives nothing secret back. On this pairing connection only, keys derive from `pair_secret` alone. The phone then generates a 32-byte `phone_secret` and sends it in its first encrypted frame (`pairComplete`, see Transport); from then on every long-term key derives from `pair_secret ‖ phone_secret`:
   - `auth_key` (per-connection challenge-response),
   - `frame_key` (E2E frame encryption, see below), and `push_key` from it.
4. Pending entry becomes active when the gateway has stored `phone_secret`. The QR is now useless: scanning it again fails (the app's first connection after a scan sends `pairing:true`, accepted only for a pending, unexpired device), and a normal handshake with keys derived from the link alone fails too, because paired keys need `phone_secret`. The link left in terminal scrollback therefore authenticates nobody after pairing; it only matters to someone who also captured the pairing connection's traffic. Two phones racing with one QR: activation is committed under the store lock, so the first `pairComplete` wins and the other is closed with 4401. A pairing connection that drops before `pairComplete` leaves the device pending (the QR still works until it expires).
- Storage: `~/.chrome-use/mobile/devices.json` + `keys/<deviceId>.key` (raw `pair_secret`, 32 bytes, while pending; `pair_secret ‖ phone_secret`, 64 bytes, once paired; a key file of the wrong length for the device's state authenticates nobody); `$CHROME_USE_RELAY_DIR/mobile` instead when that variable is set, mirroring `agent-panel`. Changes run under `store.lock`, so `pair`, `revoke` and a running gateway do not lose each other's writes. Files: dir 0700, files 0600, via `private_tree` / `atomic_write` (`agent_panel/config_transaction.rs:670, 772`); Windows user-only ACL. Phone: iOS Keychain / Android Keystore.
- All secret comparisons are constant-time (the panel service's `!=` at `service.rs:3548` must not be copied). The gateway compares only the phone proof, with `Mac::verify_slice`.
- `mobile devices [--json]` lists id, name, state (pending/expired/active), created, last seen, last IP, push on/off, and whether full access is allowed (`fullAccessAllowed`). `mobile access <id> full|limited` changes the latter (see Phone protocol); `mobile pair --allow-full-access` sets it at pairing. `mobile revoke <id>` deletes the record, key and relay token; a running gateway re-reads the store every 500 ms and closes that device's connections with code 4403.

## Transport and encryption
- Key schedule (HKDF-SHA256, RFC 5869, 32-byte output; `S = "chrome-use mobile v1"`). `IKM` is `pair_secret` (32 bytes) on the pairing connection and `pair_secret ‖ phone_secret` (64 bytes) for a paired device:
  - `auth_key = HKDF(ikm = IKM, salt = S, info = "auth")`
  - `frame_key = HKDF(ikm = IKM, salt = S, info = "frame")`
  - `push_key = HKDF(ikm = frame_key, salt = empty (32 zero bytes), info = "push")`
  - `c2p_key = HKDF(ikm = frame_key, salt = gatewayNonce‖phoneNonce, info = "c2p")` (gateway → phone), `p2c_key` likewise with `"p2c"` (phone → gateway).
- One WebSocket per phone, any path. Handshake (text frames, JSON; binary values are base64url without padding):
  1. gateway → `{"type":"challenge","v":1,"gatewayNonce":<32 bytes>,"gateway":<fp>}`
  2. phone → `{"type":"auth","v":1,"deviceId","phoneNonce":<32 random bytes>,"proof","pairing":bool}`, `proof = HMAC-SHA256(auth_key, "chrome-use mobile v1 phone proof" ‖ gatewayNonce ‖ phoneNonce ‖ utf8(deviceId))`
  3. gateway → `{"type":"ready","proof"}`, `proof = HMAC-SHA256(auth_key, "chrome-use mobile v1 gateway proof" ‖ gatewayNonce ‖ phoneNonce ‖ utf8(deviceId))`. The phone must verify it before sending anything encrypted.
  4. Pairing connection only (`pairing:true`), still within the 5 s pre-auth deadline: the phone's first encrypted frame (pairing `p2c_key`, sequence 0) is `{"id":<any>,"op":"pairComplete","params":{"phoneSecret":<32 random bytes, base64url>}}`. The gateway stores it, activates the device and answers under pairing `c2p_key`, sequence 0: `{"id":<same>,"result":{"paired":true}}`. After this acknowledgement both sides switch, on the same connection, to `c2p`/`p2c` keys derived from the paired `frame_key` with the same nonces, and both sequence numbers start again at 0. Anything else as the first frame closes the connection (4400); a device that is no longer pending (another phone won, revoked, expired) gets 4401 instead of the acknowledgement.
  - Changed from the first draft (`HMAC(auth_key, nonce‖deviceId)`): the proof binds the phone's nonce too (it salts the session keys), and the two directions use different labels so neither proof can be reflected.
  - Refusal is a close frame only, never a reason: 4401 (unknown/wrong key/wrong mode/revoked), 4429 (backoff, or too many unauthenticated connections from the source), 4408 (upgrade, handshake and pairing not finished within 5 s of the TCP connection), 4400 (malformed). One exception: a device that proves its key but is paired to another profile than a `serve --profile` gateway accepts gets 4409 with reason "device is paired to another profile", and this does not count toward backoff. After the handshake: 4400 for a rejected frame, 4403 when the device is revoked.
- Every frame after the handshake: AES-256-GCM (available natively in CryptoKit, Android `javax.crypto` and the existing `aes-gcm` crate). Per-connection keys `HKDF(frame_key, salt = gatewayNonce‖phoneNonce, info = "c2p"|"p2c")`, one per direction; nonce = 4 zero bytes ‖ u64 BE sequence number, starting at 0 and strictly increasing, so nonces never repeat under one key. Each WebSocket binary message is `seq (8 bytes, BE) ‖ ciphertext ‖ tag (16)`, with the same 8 sequence bytes as associated data. A frame whose sequence is not exactly the next expected one, or that fails authentication, closes the connection (4400). Text frames after the handshake also close it. Plaintext is UTF-8 JSON. Messages are capped at 2 MiB.
- Push payloads use `push_key` with a random 12-byte nonce carried in the payload, no associated data (pushes are not on a connection).
- Rationale: Cloudflare tunnels terminate TLS at the edge, so TLS alone would let the tunnel provider read page content and instructions. Both ends are the user's devices, so a symmetric key carried in the QR is sufficient; no PKI.
- Rate limit (in memory only): failed handshakes per source and per (source, claimed device id). A source is an IPv4 address or an IPv6 /64 (IPv4-mapped IPv6 counts as IPv4). The first 2 failures are free; from the 3rd, the source (or that source's tries for that device) waits 1 s, doubling per failure up to 300 s; a success clears both. Upgrade and handshake timeouts count as failures. A blocked source is dropped before the WebSocket upgrade or closed with 4429. Each table keeps at most 4096 entries, evicting the least recently touched. A device is never disabled by failures (anyone who knows its id could cause them), and devices of another profile on a `--profile` gateway are not counted. Limits: 32 connections at once, 4 unauthenticated connections per source, 5 s from TCP accept to the end of the handshake (and pairing).

## Phone protocol (inside encrypted frames)
`{id, op, params}` → `{id, result}` | `{id, error:{code, message}}`; `id` is echoed as sent. Error codes: `denied` (op or field not allowed from the phone; never forwarded), `accessTooHigh` (see below), `invalidParams`, `notFound`, `limit`, `rateLimited`, `panel` (the panel service refused; `message` is its text), `screenshot`, `store`. Up to 8 requests run concurrently per connection; more wait.

Server events:
- `{event:"journal", conversationId, conversation, events, cursor, more, queuedMessages}`: the `conversationRead` result for a subscription, sent when there are new events or the conversation summary changed. `cursor` is the value to resume from.
- `{event:"subscriptionError", conversationId, error}` once per run of failed polls (the service is not running, …); polling continues every 3 s.
- `{event:"subscriptionEnded", conversationId, reason}` when the conversation no longer exists.
- `{event:"pushTokenRejected", reason:"invalid"|"expired"|"unregistered"}` to every live connection of a device whose relay token the Worker just refused (401 `invalid_token` / `expired_token`, 410). The app should register again with a new token.

Allowed ops (mapped 1:1 to panel operations unless noted):
- `discover`, `conversationList`, `scheduleList`: `{}` only.
- `conversationCreate {agentId, model?, options?, fullAccess?}`: `fullAccess` may only be `false` and is always sent as `false`. The effective tool access (`toolAccess`, else the agent default: OpenCode → runCommands, Codex → readFiles, others → browserOnly) must be at most `writeFiles`, so `runCommands` and OpenCode conversations are created on the desktop only. Pi never asks before a tool runs, so a Pi conversation from the phone must be `browserOnly` (the panel then gives it the browser tools only; `backend.rs`, and `schedule_permissions_fit` treats Pi as automatic). `model` must not start with `-`.
- Elevated conversations: a conversation or schedule is elevated when it has `fullAccess`, an effective `runCommands` (including every OpenCode one), or is Pi above `browserOnly` — it then acts without asking or can run commands. Each device has `allowFullAccess` (default false), set only on the computer (`mobile pair --allow-full-access`, `mobile access <id> full|limited`). Without it, these are refused for an elevated conversation/schedule with error `accessTooHigh` (the message tells the user to enable it on the computer): `conversationSend`, `conversationSendDirect`, `conversationRespond` with an approving decision (anything other than `"deny"`, `"decline"`, `"cancel"`; ACP option ids and question answers count as approving), `conversationSetOptions`, `scheduleRun`. Always allowed whatever the access: `conversationStop`, declining answers, `conversationRead`, `subscribe`, `screenshot`. With it, an elevated conversation is driven with its own access, never raised. `hello.fullAccessAllowed` tells the app, which then requires its biometric/passcode lock. The flag is read from the store on every request, so `mobile access` takes effect immediately.
- `conversationSend` / `conversationSendDirect {conversationId, messageId, text, attachments?, record?}`: `record` only `false` (dropped); at most 5 attachments, each `bytes` ≤ 512 KiB (the service re-verifies the files).
- `conversationStop {conversationId}`, `conversationRead {conversationId, after?}` (initial page).
- `conversationRespond {conversationId, requestId, generation, requestRevision, decision}`: all five required; the legacy `result` form is refused.
- `conversationAttachmentPut {conversationId, data}`: base64 length re-checked against 512 KiB before forwarding.
- `conversationDelete {conversationId, confirm:true}`: forwarded as `{conversationId}`.
- `conversationSetOptions {conversationId, options:{effort?, serviceTier?, toolAccess?}}`: options replace each other whole, so when the phone omits `toolAccess` the gateway sends the conversation's current one. Refused if the new effective tool access is higher than the conversation's real current one (read from `conversationList`), and for Pi above `browserOnly`.
- `scheduleRun {id}`; `scheduleSave {id, enabled}` exactly, for a schedule that already exists (checked with `scheduleList`, since `scheduleSave` with an unknown id would create one). A refusal from the service (for example an unconfirmed previous run that must be acknowledged on the desktop) is passed through.
- Conversation and schedule ids must be UUIDs.
- `subscribe {conversationId, after?}` / `unsubscribe {conversationId}` — gateway-side polling of `conversationRead` every 600 ms while the conversation is busy (starting/running/waitingInput/stopping), 1800 ms otherwise, immediately again while `more` is true. At most 16 per connection; subscribing again to the same conversation restarts it from the new `after`. Subscriptions end with the connection.
- `pushRegister {relayToken}` / `pushUnregister {}` — stores the push relay token for this device (see Notifications) → `{registered:true}`. If the Worker has already refused that exact token, nothing is stored and the reply is `{registered:false, reason}` (`invalid` | `expired` | `unregistered`); the app should get a new token from `/v1/register`. The app should send `pushRegister` on every connection.
- `hello {}` → `{computerName, gatewayVersion, pushMutes, fullAccessAllowed}`: the computer's display name (as in the QR `n`, current value), the chrome-use version of the gateway, this device's muted conversation ids, and whether it may drive elevated conversations. Sent inside the encrypted channel, so it is authenticated, unlike the plaintext `ready` frame.
- `pushMute {conversationId, muted}` — per-device mute, stored in `devices.json` (at most 512 per device); the push watcher sends that device nothing for a muted conversation. Notices without a conversation (`test`) are never muted.
- `agentInspect {agentId}` (`codex` | `claude` | `opencode` | `pi`) — the agent CLI's model list for the create screen. It runs the installed CLI's metadata probe (`--version`, then its protocol's model listing in a deleted scratch folder; OpenCode's transient ACP session is deleted) and changes no configuration. `executable` is refused (it would run any program). At most one real inspection per agent and profile every 30 s; within that window the previous result or refusal is returned again, and a request while one is running gets `rateLimited`.
- `screenshot {conversationId}` — the conversation's browser session is `panel-<conversationId>` (the panel service's own name for it: `summary().target.session`, used by `close_browser_session` / `adopt_granted_tab`). The conversation must be in this device's profile. The gateway sends one `screenshot` command straight to that session's running daemon socket (`connection::send_to_running_daemon`: no daemon start, no stale-daemon kill, no retry); with no daemon the request fails, so a phone never starts a daemon or opens a browser. (`chrome-use screenshot` as a subprocess would start a daemon when none is running.) Result `{mediaType:"image/jpeg", width, height, data:<base64>}`, at most 1080 px wide, JPEG quality 70; ≤1 per 2 s per conversation (`rateLimited`); no running session → error `screenshot`.

Op results (`result` of a successful reply). Panel-forwarded results are the panel service's own objects, passed through unchanged; they all carry `schemaVersion:1`. `Conversation` is the panel summary `{id, agentId, title, created, updated, phase, control, generation, model, requestedModel, options, queued, queuePaused, directMessageId, activeMessageId, lastError, lastSequence, fullAccess, site, scheduleId, lastTurn:{messageId, outcome, ended, preview?}|null, waitingRequest, target:{mode, session}, project?, resumed?}`; `phase` is one of `idle` `starting` `running` `waitingInput` `stopping` `failed` `interrupted` `recoveryRequired`. `Schedule` is `{id, name, prompt, agentId, model, options, fullAccess, rule:{kind:"interval",minutes}|{kind:"daily"|"weekdays",time:"HH:MM"}|{kind:"cron",expression}, enabled, nextRun, skillId, activeChat, lastChat, lastResult, lastRun, finishedAt}` (times are Unix ms).

| Op | `result` |
|---|---|
| `hello` | `{computerName, gatewayVersion, pushMutes:[conversationId], fullAccessAllowed}` |
| `discover` | `{schemaVersion, agents:[{id, name, executable, configDirectory, configFiles, directoryEvidence, authentication, runtime, warnings}]}` |
| `agentInspect` | `{schemaVersion, inspection:{agentId, executable, versionReported, source, observedAt, models:[{id, label, efforts, isDefault, supportsFastMode}], complete, account:{status}, modelAccessVerified, processExitConfirmed}}` (model fields vary slightly by agent; `id` and `label` are always there) |
| `conversationList` | `{schemaVersion, conversations:[Conversation]}` (most recently updated first) |
| `conversationCreate` | `{schemaVersion, conversation}` |
| `conversationSend`, `conversationSendDirect` | `{schemaVersion, conversation, message:{id, accepted, replayed}}`; `accepted:false` means this message id was already accepted (a retry), not a refusal |
| `conversationStop`, `conversationSetOptions` | `{schemaVersion, conversation}` |
| `conversationRespond` | `{schemaVersion, conversation, response:{dispatched:true, confirmed:false}}`; the outcome shows up in the journal |
| `conversationRead` | `{schemaVersion, conversation, journal:{events:[{sequence, at, kind, data}], cursor, more}, queuedMessages:[{id, text, truncated, attachments, direct}]}` |
| `conversationAttachmentPut` | `{schemaVersion, conversation, attachment:{id, mediaType, bytes}, data:null}`; pass `attachment` as-is in `conversationSend.attachments` |
| `conversationDelete` | `{schemaVersion, deleted:{id, cliHistory:{…}, browserSession:{…}}}` |
| `scheduleList` | `{schemaVersion, schedules:[Schedule], timeZone:"hostLocal", serviceRunning}` |
| `scheduleRun` | `{schemaVersion, schedule, run:{conversationId, dispatched:true, completed:false}}` |
| `scheduleSave` (`{id, enabled}`) | `{schemaVersion, schedule}` |
| `subscribe` | `{subscribed:conversationId, after}`; events follow as `journal` frames |
| `unsubscribe` | `{unsubscribed:bool}` (false: there was no such subscription) |
| `pushRegister` | `{registered:true}` or `{registered:false, reason:"invalid"\|"expired"\|"unregistered"}` |
| `pushUnregister` | `{registered:false}` |
| `pushMute` | `{conversationId, muted, pushMutes:[conversationId]}` |
| `screenshot` | `{mediaType:"image/jpeg", width, height, data:<base64>}` |

Denied from the phone in v1:
- `conversationSetAccess` / any `fullAccess=true`, `conversationProjectChoose` (opens a native dialog on the desktop), `serviceStop`, skill writes/imports, schedule create/edit beyond `enabled`, `config*`, recovery/evidence ops, `conversationResume` of sessions not created from the panel.
- Added during implementation: `agentInspect.executable`, `conversationCreate.executable` (an arbitrary program path) and `.project` (an arbitrary folder); `conversationSend.page`, `.tabGrant` (lends a desktop tab), `.skillId` (needs `conversationSkillSelect`, not allowed), `record:true`; `conversationRespond.result`; `conversationAttachmentRead`, `conversationArtifactRead`, queue edits, `conversationTabGrant`, `conversationSkillSelect`, `conversationSessionList`, `conversationProjectList`, `health`/`serviceHealth`, `agent*` tool operations, inspect operations, `imageConvert`; `scheduleDelete`; raising tool access through `conversationSetOptions`.
- Any op not on the allowlist is rejected by the gateway, not forwarded.

## "Needs you" path
- v1 source: `request` events / phase `waitingInput`. The phone renders the redacted request and offers exactly the decision values valid for that backend method (`approval.rs:41-112`), echoing `generation` and `requestRevision`.
- Captcha/2FA: not wired in v1 (no panel event exists). Later phase: emit a panel event from `human_check_verdict` / login-wall detection, and let the phone answer via screenshot + tap using the stream server's `input_touch` behind the gateway.

## Notifications
Push goes through our Cloudflare Worker `chrome-use-push.leeguoo.com` (one level under `leeguoo.com`, so Cloudflare Universal SSL covers it), which holds the APNs key and FCM service account and nothing else.

```
gateway ──POST /v1/push {relayToken, ciphertext}──▶ push Worker ──▶ APNs / FCM ──▶ phone (decrypts locally)
```

- Registration: the app gets its APNs/FCM device token, calls `POST /v1/register {platform, deviceToken}` on the Worker, and receives `relayToken = MAC(worker_secret, platform‖deviceToken‖issuedAt)`. The Worker stores nothing; the token is self-verifying. The app sends `relayToken` to the gateway over the encrypted channel (`pushRegister` op); the gateway stores it with the device record.
- Content is end-to-end encrypted: the gateway encrypts `{v, conversationId, kind, title, body}` with `push_key` (derived from `frame_key`); the Worker forwards the opaque blob.
  - iOS: `mutable-content: 1` alert with a generic fallback ("chrome-use needs you"); a Notification Service Extension decrypts and rewrites title/body. The key is shared with the extension via an App Group keychain entry.
  - Android: FCM data message; the app decrypts and posts the notification itself.
- Triggers: `waitingInput` (high priority, time-sensitive on iOS), turn end (`terminal`, normal priority), scheduled run finished/failed. Per-conversation mute on the phone; the gateway debounces to ≤1 push per conversation per 10 s.
- Gateway watcher (implemented): every 2 s, for each profile that has an active device with a relay token (only the `--profile` one on a filtered gateway), it polls `conversationList` and `scheduleList` of the running service (never starting one). The first poll of a profile is a baseline and announces nothing. Then:
  - a conversation entering `waitingInput` (from any other phase, or new) → `needsInput` (code `needsInput`), `high`, title = conversation title (first line, ≤80 chars), body `"Waiting for your approval"` (the request itself is not read for the push);
  - a conversation leaving a busy phase for a non-busy one, without `scheduleId` → `turnEnded` (code `finished` / `stopped` / `failed` by the new phase), `normal`, body = `lastTurn.preview` (already redacted, ≤120 chars) or Finished/Stopped/Failed;
  - a schedule whose `finishedAt` changed → `scheduleDone` (`lastResult == "completed"`) or `scheduleFailed` (body = `lastResult`, ≤120 chars), conversationId = `lastChat`, title = schedule name.
  - Debounce key is the conversation id (the schedule name when there is none). A notice inside the 10 s window waits and replaces the waiting one, except that a waiting `needsInput` is not replaced by a lesser kind; it goes out when the window closes.
  - `collapseId` = conversation id. Worker replies: 410 (`unregistered`), 401 `expired_token` (`expired`) or any other 401 (`invalid`) remove that token from the device and remember it as rejected with that reason in `devices.json` (`rejectedToken`, `rejectedReason`), so registering it again is refused; registering a different token clears the record. A 410 body may name the refusing service's reason (`BadDeviceToken`, `Unregistered`, `UNREGISTERED`, `NOT_FOUND`); all mean the token is dead. A 502 `{status:"upstream_error", reason:"misconfigured"}` means the Worker lacks its APNs/FCM credentials: the gateway prints a clear line to stderr and keeps the token. Connected phones get `pushTokenRejected`; 429 pauses pushes to that device for `retryAfter` seconds; other failures are logged.
  - Worker URL: `--push-url`, else `CHROME_USE_PUSH_URL`, else `https://chrome-use-push.leeguoo.com`.
- `chrome-use mobile push-test <deviceId>` sends `kind:"test"` (title "chrome-use", body "Test notification (<unix time>)", empty conversationId, no collapseId). Apps should treat unknown kinds like `test`: show title/body, open the app.
- What a push contains (end-to-end encrypted; the Worker and APNs/FCM see only ciphertext): the conversation title (≤80 chars, from the user's first message) or schedule name, and a body that is either a fixed text, the schedule's result line, or the first line of the agent's last answer as redacted by the panel (≤120 chars). The approval request itself, page content and tool output are not included.
- Abuse limits in the Worker: per-minute burst caps via the Workers Rate Limiting binding: per `relayToken` (10/60 s), per source IP on push (120/60 s) and on register (10/60 s); `relayToken` (the gateway accepts up to 8192 characters) expires after 90 days and the app silently refreshes it. APNs `410 Unregistered` / FCM `UNREGISTERED` is returned to the gateway, which drops the token.
- Revocation: `mobile revoke` deletes the device's `relayToken` on the gateway; the Worker never needs to know.
- Cost: Workers free tier (100k requests/day) covers early usage; no KV/D1/DO needed. Track with the cloudflare-cost-aware rules if a store is ever added.
- Worker API (JSON over HTTPS):
  - `POST /v1/register {platform: "ios"|"android", deviceToken, env?: "sandbox"|"production"}` → `{relayToken, expiresAt}`. `relayToken = base64url(JSON{p,d,e,iat,exp}) "." base64url(HMAC-SHA256(RELAY_SECRET, first part))`.
  - `POST /v1/push {relayToken, payload, priority: "high"|"normal", collapseId?}` → `200 {status:"sent"}` | `401 {status:"invalid_token"|"expired_token"}` | `410 {status:"unregistered"}` | `429 {status:"rate_limited", retryAfter}` | `502 {status:"upstream_error"}`.
  - `payload` = padded standard base64 (at least 32 decoded bytes, which kid + nonce + tag always are) of `kid(4) ‖ nonce(12) ‖ AES-256-GCM ciphertext+tag` of UTF-8 JSON `{v:1, conversationId, kind:"needsInput"|"turnEnded"|"scheduleDone"|"scheduleFailed"|"test", code, title, body}` under the push key, no associated data; ≤ 2048 characters. The Worker never parses it. `kid` = first 4 bytes of `HMAC-SHA256(push_key, "chrome-use mobile v1 kid")`: not secret; a phone paired with several computers (or a Notification Service Extension) uses it to pick the key, and drops a payload whose kid matches none.
  - `code` is the stable reason to localize by, never the English `body`: `needsInput`, `finished` (turn ended idle), `stopped` (interrupted), `failed` (failed or unconfirmed), `scheduleDone`, `scheduleFailed`, `test`. `title` is the conversation title or schedule name; `body` is the turn's redacted preview, the schedule's result, or an English fallback.
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
- Resolved: the QR link is no longer a long-term credential (`phone_secret` rotation, above).
- Resolved: the panel service's journal read cache kept one offset per conversation, matched only at its exact cursor, so the gateway and the side panel polling one conversation at different cursors rescanned `events.jsonl` from the start on every read. It now keeps up to 4 verified prefixes per conversation and resumes from the furthest one at or before the requested cursor (`conversation.rs`, `Store::events`).
- Decided: the `needsInput` push body stays generic for privacy.
- Decided: phone-created conversations are capped at `writeFiles`; OpenCode is not creatable from the phone in v1. Commands then reach the phone as approval requests.
- The integration test is Unix-only (Windows would need the ACL fixture used by `agent_panel_native.rs`).

## Appendix: test vectors
All values hex unless noted. Inputs: `pair_secret = 00 01 … 1f`, `phone_secret = 80 81 … 9f` (base64url `gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8`), `gatewayNonce = 20 21 … 3f`, `phoneNonce = 40 41 … 5f`, `deviceId = "0f8fad5b-d9cb-469f-a165-70867728950e"`. Checked by `mobile::crypto::tests::spec_test_vectors` and computed independently with Python `cryptography`.

Pairing connection (`IKM = pair_secret`):

| Value | Result |
|---|---|
| `auth_key` | `a8344e0caf1c7d238af084ccdbeb9d4c2d8ac1abeea4314ca22d1d3aaa47e00e` |
| `frame_key` | `1ab8cb618f09a935128d1aca5e0aa708610fd20cb23a3b473e2446052036400d` |
| `c2p_key` | `5bc1cde1dbe85a0bafb5c37868e4b8798a5d290b548b2c8eea7831449f33116a` |
| `p2c_key` | `5f2d6790a8083d40200190969fa564fae1390dacab76bd4318c41185f10a6845` |
| phone proof | `8dc9b0be57af6d6329053ba1807835eb333d617132c1f1c00d494fa21b99ec20` (base64url `jcmwvlevbWMpBTuhgHg16zM9YXEywfHADUlPohuZ7CA`) |
| gateway proof | `7fcae67185c35186b70f2434a8ba5d593d40b2ea074558601cdd76b1e43f34e6` |

`pairComplete` frame (p2c, sequence 0), plaintext `{"id":0,"op":"pairComplete","params":{"phoneSecret":"gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8"}}`:

```
0000000000000000eecbbe0b87d98d8799928139b1306a0e8ad641630d702f3912455ee905df518bc47e2a62a8fbf37d44334e959fba8654b2a8ec58ee735b6d9d77cf5a8754981d6b520b9cec3c24c249fcdda6072bd7903904448ba4062b027abc2248c9370df66d6cc1bca2459aebcb0187444d0f2ed86431c5
```

Acknowledgement (c2p, sequence 0), plaintext `{"id":0,"result":{"paired":true}}`:

```
00000000000000002887dc6b0a83e1b96fccac5816546874f376ca967dc9dd031675a37003e6d635896c1495a11d67ef35831329b779c306a8
```

Paired device (`IKM = pair_secret ‖ phone_secret`, 64 bytes), same nonces:

| Value | Result |
|---|---|
| `auth_key` | `1ff8540b97d115a12bf80351302fc1b97847bf21ad52bef5b46a2e305c1c5ddc` |
| `frame_key` | `624f1577d77bcf36468bd37c6acd886a73fce2796128b9130416ec88849f46b3` |
| `push_key` | `62ea56e406bb35ebf28f9c403bd6bfd0431aef952cd6d13d6ac50de785e0b621` |
| `c2p_key` | `691a14378649adfe6b24fed30a8e12a6465b064ee71bc7f5eba952c04fd2b5b7` |
| `p2c_key` | `9ae2fd1ae48966a3576dc7b61f2d8b6f59e9913596062e1940aa432dbc46d024` |
| phone proof | `a2766a2b56dfecf634d78a5ff550d23f073cd6bc6a1b66538b0f27461f9c23c3` |
| gateway proof | `3f56ef6947151350afa20a71ef7dfe4af26240a5e3d4814c6292005051e5b55b` |

First phone frame of a paired connection (p2c, sequence 0), plaintext `{"id":1,"op":"conversationList","params":{}}`:

```
00000000000000003d3fea079d6d776461ebe1024f75712d9eb0b8324463906d3735ad868274cd09d2ed59f4fc423448e2dccb847c9f6b202838c5922a0c6c496cdb5726
```

Push under the paired `push_key`: `kid` = `0fd39a46`. With nonce `60 61 … 6b` and plaintext
`{"v":1,"conversationId":"7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e","kind":"needsInput","code":"needsInput","title":"Weekly report","body":"Waiting for your approval"}` the payload (standard base64 of kid ‖ nonce ‖ ciphertext ‖ tag) is:

```
D9OaRmBhYmNkZWZnaGlqa5LWr9sMfSfsQKyuDrzA3lU/VPPJ6gqLhPXaVYvov7CkcK80I1Lyu2LV3h1400pohwLJxAkRdNJDpBOB6YFXBj+KqnEkTNyedZ9NlyimW4JZCLDWHVkisCr6vRg+bP0LTJ9IPNZ/vuEAUEvASnRUoP0fA2z1ChvjIk0tBepBrmiuVxuePSZeJEuygSCkNnBbEVQAqk2LryQAdNZH2Fn72U5ucwevM2p9auOluknJ1H1AWrc=
```
