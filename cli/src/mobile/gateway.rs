//! `chrome-use mobile serve`: the WebSocket endpoint a paired phone connects
//! to. Plain JSON is exchanged only for the handshake; every later frame is
//! AES-256-GCM under per-connection keys (see `crypto`). Requests are checked
//! by `policy` and reach the panel service through `panel` only.

use super::crypto::{self, Channel, DeviceKeys, NONCE_LEN};
use super::panel::Panel;
use super::policy::{self, Action, Refusal};
use super::push;
use super::store::{self, State, Store};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch, Semaphore};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

const MAX_CONNECTIONS: usize = 32;
const MAX_MESSAGE: usize = 2 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const REQUESTS_IN_FLIGHT: usize = 8;
const MAX_SUBSCRIPTIONS: usize = 16;
const SCREENSHOT_INTERVAL: Duration = Duration::from_secs(2);
const SCREENSHOT_WIDTH: u32 = 1080;
/// Failures from one source before backoff starts, and the longest backoff.
const FREE_FAILURES: u32 = 3;
const MAX_BACKOFF: Duration = Duration::from_secs(300);

const CLOSE_PROTOCOL: u16 = 4400;
const CLOSE_UNAUTHORIZED: u16 = 4401;
const CLOSE_REVOKED: u16 = 4403;
const CLOSE_TIMEOUT: u16 = 4408;
/// A paired device of another profile reached a `--profile` gateway.
const CLOSE_OTHER_PROFILE: u16 = 4409;
const CLOSE_RATE_LIMITED: u16 = 4429;

pub struct Options {
    pub bind: SocketAddr,
    /// Accept only devices paired to this panel profile.
    pub profile: Option<String>,
    pub push_url: String,
    /// Overrides (and saves) the computer name shown to phones.
    pub name: Option<String>,
}

#[derive(Default)]
struct Strike {
    count: u32,
    until: Option<Instant>,
}

/// Failed handshakes per source address and per claimed device, with
/// exponential backoff. Persistent per-device counts live in the store.
#[derive(Default)]
pub(crate) struct Limiter {
    sources: HashMap<IpAddr, Strike>,
    devices: HashMap<String, Strike>,
}

impl Limiter {
    fn wait<K: std::hash::Hash + Eq>(map: &HashMap<K, Strike>, key: &K, now: Instant) -> bool {
        map.get(key)
            .and_then(|s| s.until)
            .is_some_and(|until| until > now)
    }
    fn strike<K: std::hash::Hash + Eq>(map: &mut HashMap<K, Strike>, key: K, now: Instant) {
        if map.len() > 4096 {
            map.retain(|_, s| s.until.is_some_and(|u| u > now));
        }
        let strike = map.entry(key).or_default();
        strike.count = strike.count.saturating_add(1);
        if strike.count >= FREE_FAILURES {
            let exponent = (strike.count - FREE_FAILURES).min(16);
            let delay = Duration::from_secs(1u64 << exponent).min(MAX_BACKOFF);
            strike.until = Some(now + delay);
        }
    }
    pub(crate) fn source_blocked(&self, ip: IpAddr, now: Instant) -> bool {
        Self::wait(&self.sources, &ip, now)
    }
    pub(crate) fn device_blocked(&self, id: &str, now: Instant) -> bool {
        Self::wait(&self.devices, &id.to_string(), now)
    }
    pub(crate) fn fail(&mut self, ip: IpAddr, device: Option<&str>, now: Instant) {
        Self::strike(&mut self.sources, ip, now);
        if let Some(id) = device {
            Self::strike(&mut self.devices, id.to_string(), now);
        }
    }
    pub(crate) fn succeed(&mut self, ip: IpAddr, device: &str) {
        self.sources.remove(&ip);
        self.devices.remove(device);
    }
}

/// One authenticated connection: its device, close signal and event queue.
struct Live {
    device: String,
    close: watch::Sender<bool>,
    outbox: Outbox,
}

pub(crate) struct Shared {
    pub(crate) store: Store,
    pub(crate) panel: Panel,
    pub(crate) profile: Option<String>,
    limiter: Mutex<Limiter>,
    live: Mutex<HashMap<u64, Live>>,
    /// Shown to the phone by `hello`.
    computer_name: String,
    next_connection: AtomicU64,
    screenshots: Mutex<HashMap<String, Instant>>,
}

impl Shared {
    /// Queue an event for every live connection of `device`; dropped when a
    /// connection's queue is full.
    pub(crate) fn notify(&self, device: &str, event: Value) {
        if let Ok(live) = self.live.lock() {
            for connection in live.values().filter(|c| c.device == device) {
                let _ = connection.outbox.try_send(event.clone());
            }
        }
    }
}

pub async fn serve(options: Options) -> Result<(), String> {
    let store = Store::open()?;
    let computer_name = match options.name {
        Some(name) => {
            let saved = name.clone();
            store.update(|r| {
                r.computer_name = Some(saved);
                Ok(())
            })?;
            name
        }
        None => store
            .read()?
            .computer_name
            .unwrap_or_else(super::computer_name),
    };
    let listener = TcpListener::bind(options.bind)
        .await
        .map_err(|e| format!("Cannot listen on {}: {e}", options.bind))?;
    let local = listener
        .local_addr()
        .map_err(|_| "Cannot read the listening address")?;
    let shared = Arc::new(Shared {
        store,
        panel: Panel::default(),
        profile: options.profile,
        limiter: Mutex::new(Limiter::default()),
        live: Mutex::new(HashMap::new()),
        computer_name,
        next_connection: AtomicU64::new(1),
        screenshots: Mutex::new(HashMap::new()),
    });
    eprintln!("mobile gateway listening on ws://{local}");
    // Machine-readable readiness line for scripts and tests.
    println!("{}", json!({"listening": local.to_string()}));
    tokio::spawn(watch_revocations(shared.clone()));
    tokio::spawn(push::watch(shared.clone(), options.push_url));
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        };
        if shared
            .limiter
            .lock()
            .map(|l| l.source_blocked(peer.ip(), Instant::now()))
            .unwrap_or(true)
        {
            continue;
        }
        let shared = shared.clone();
        tokio::spawn(async move {
            let _slot = slot;
            connection(shared, stream, peer).await;
        });
    }
}

/// Drop connections of revoked or disabled devices within half a second.
async fn watch_revocations(shared: Arc<Shared>) {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let store = shared.store.clone();
        let Ok(Ok(registry)) = tokio::task::spawn_blocking(move || store.read()).await else {
            continue;
        };
        if let Ok(live) = shared.live.lock() {
            for Live { device, close, .. } in live.values() {
                if registry
                    .device(device)
                    .is_none_or(|d| d.state != State::Active)
                {
                    let _ = close.send(true);
                }
            }
        }
    }
}

async fn close(ws: &mut WebSocketStream<TcpStream>, code: u16, reason: &'static str) {
    let _ = ws
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.into(),
        })))
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(1), ws.close(None)).await;
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

// The upgrade callback's error type is tungstenite's HTTP response.
#[allow(clippy::result_large_err)]
async fn connection(shared: Arc<Shared>, stream: TcpStream, peer: SocketAddr) {
    let mut forwarded: Option<IpAddr> = None;
    let config = WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE),
        max_frame_size: Some(MAX_MESSAGE),
        ..Default::default()
    };
    // A tunnel on this machine connects from loopback; its client address
    // header is then the only per-source identity there is.
    let trust_forwarded = is_loopback(peer.ip());
    let upgrade = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        |request: &Request, response: Response| {
            if trust_forwarded {
                forwarded = request
                    .headers()
                    .get("cf-connecting-ip")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse().ok());
            }
            Ok(response)
        },
        Some(config),
    );
    let Ok(Ok(mut ws)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, upgrade).await else {
        return;
    };
    let source = forwarded.unwrap_or(peer.ip());
    if shared
        .limiter
        .lock()
        .map(|l| l.source_blocked(source, Instant::now()))
        .unwrap_or(true)
    {
        close(&mut ws, CLOSE_RATE_LIMITED, "try again later").await;
        return;
    }
    let session =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&shared, &mut ws, source)).await {
            Ok(Ok(session)) => session,
            Ok(Err(code)) => {
                let reason = if code == CLOSE_OTHER_PROFILE {
                    "device is paired to another profile"
                } else {
                    "handshake failed"
                };
                close(&mut ws, code, reason).await;
                return;
            }
            Err(_) => {
                close(&mut ws, CLOSE_TIMEOUT, "handshake timed out").await;
                return;
            }
        };
    let (closer, closed) = watch::channel(false);
    let connection_id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
    let (outbox, queue) = mpsc::channel::<Value>(64);
    if let Ok(mut live) = shared.live.lock() {
        live.insert(
            connection_id,
            Live {
                device: session.device.clone(),
                close: closer,
                outbox: outbox.clone(),
            },
        );
    }
    eprintln!("mobile: device {} connected from {source}", session.device);
    let code = run_session(&shared, ws, session.clone(), closed, outbox, queue).await;
    if let Ok(mut live) = shared.live.lock() {
        live.remove(&connection_id);
    }
    eprintln!("mobile: device {} disconnected ({code})", session.device);
}

#[derive(Clone)]
struct Session {
    device: String,
    profile: String,
    send_key: [u8; 32],
    receive_key: [u8; 32],
}

/// Run the challenge-response. On failure the reply is a close code only; it
/// never says whether the device exists.
async fn handshake(
    shared: &Arc<Shared>,
    ws: &mut WebSocketStream<TcpStream>,
    source: IpAddr,
) -> Result<Session, u16> {
    let registry = {
        let store = shared.store.clone();
        tokio::task::spawn_blocking(move || store.read())
            .await
            .map_err(|_| CLOSE_PROTOCOL)?
            .map_err(|_| CLOSE_PROTOCOL)?
    };
    let gateway_nonce: [u8; NONCE_LEN] = crypto::random().map_err(|_| CLOSE_PROTOCOL)?;
    let challenge = json!({"type":"challenge","v":1,"gatewayNonce":crypto::b64url(&gateway_nonce),"gateway":registry.gateway_id});
    ws.send(Message::Text(challenge.to_string()))
        .await
        .map_err(|_| CLOSE_PROTOCOL)?;
    let reply = loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => break text,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return Err(CLOSE_PROTOCOL),
        }
    };
    let reply: Value = serde_json::from_str(&reply).map_err(|_| CLOSE_PROTOCOL)?;
    let device_id = reply["deviceId"].as_str().unwrap_or("").to_string();
    let pairing = reply["pairing"] == json!(true);
    let phone_nonce = reply["phoneNonce"]
        .as_str()
        .and_then(crypto::from_b64url::<NONCE_LEN>);
    let proof = reply["proof"].as_str().and_then(crypto::from_b64url::<32>);
    let now = Instant::now();
    let fail = |known: bool| {
        if let Ok(mut limiter) = shared.limiter.lock() {
            limiter.fail(source, known.then_some(device_id.as_str()), now);
        }
    };
    if reply["type"] != "auth" || reply["v"] != 1 {
        fail(false);
        return Err(CLOSE_PROTOCOL);
    }
    let (Some(phone_nonce), Some(proof)) = (phone_nonce, proof) else {
        fail(false);
        return Err(CLOSE_UNAUTHORIZED);
    };
    let wall = store::now_ms();
    let device = registry.device(&device_id).cloned();
    let Some(device) = device.filter(|d| d.usable(wall)) else {
        fail(false);
        return Err(CLOSE_UNAUTHORIZED);
    };
    if shared
        .limiter
        .lock()
        .map(|l| l.device_blocked(&device.id, now))
        .unwrap_or(true)
    {
        return Err(CLOSE_RATE_LIMITED);
    }
    // A pending device is only reachable by pairing, and pairing only a
    // pending device: scanning the same QR again fails.
    let mode_ok = pairing == (device.state == State::Pending);
    // Pending keys come from the pair secret alone, paired keys also from the
    // phone's secret; a key file of the other kind authenticates nobody.
    let expected_len = if device.state == State::Pending {
        crypto::KEY_LEN
    } else {
        2 * crypto::KEY_LEN
    };
    let secret = shared
        .store
        .secret(&device.id)
        .ok()
        .filter(|s| s.len() == expected_len);
    let keys = secret.as_deref().map(DeviceKeys::derive);
    let verified = keys.as_ref().is_some_and(|keys| {
        crypto::verify_phone_proof(keys, &gateway_nonce, &phone_nonce, &device.id, &proof)
    });
    if !verified {
        fail(true);
        let store = shared.store.clone();
        let id = device.id.clone();
        let _ = tokio::task::spawn_blocking(move || {
            store.update(|r| {
                if let Some(d) = r.device_mut(&id) {
                    d.failures = d.failures.saturating_add(1);
                    if d.failures >= store::MAX_FAILURES {
                        d.state = State::Disabled;
                    }
                }
                Ok(())
            })
        })
        .await;
        return Err(CLOSE_UNAUTHORIZED);
    }
    // A genuine device of another profile is not an attack: it is told so
    // and does not count toward backoff.
    if shared
        .profile
        .as_ref()
        .is_some_and(|p| p != &device.profile)
    {
        return Err(CLOSE_OTHER_PROFILE);
    }
    if !mode_ok {
        fail(true);
        return Err(CLOSE_UNAUTHORIZED);
    }
    let (Some(keys), Some(secret)) = (keys, secret) else {
        return Err(CLOSE_UNAUTHORIZED);
    };
    let ready = json!({"type":"ready","proof":crypto::b64url(&crypto::gateway_proof(&keys, &gateway_nonce, &phone_nonce, &device.id))});
    let ip = source.to_string();
    let keys = if pairing {
        ws.send(Message::Text(ready.to_string()))
            .await
            .map_err(|_| CLOSE_PROTOCOL)?;
        let phone_secret = complete_pairing(
            shared,
            ws,
            &keys,
            &gateway_nonce,
            &phone_nonce,
            &device.id,
            ip,
        )
        .await?;
        let mut ikm = secret;
        ikm.extend_from_slice(&phone_secret);
        DeviceKeys::derive(&ikm)
    } else {
        // Committed under the lock, so a revoke racing this handshake wins.
        let seen = {
            let store = shared.store.clone();
            let id = device.id.clone();
            tokio::task::spawn_blocking(move || {
                store.update(|r| {
                    let d = r.device_mut(&id).ok_or("revoked")?;
                    if d.state != State::Active {
                        return Err("state changed".to_string());
                    }
                    d.failures = 0;
                    d.last_seen = Some(store::now_ms());
                    d.last_ip = Some(ip);
                    Ok(())
                })
            })
            .await
        };
        if !matches!(seen, Ok(Ok(()))) {
            return Err(CLOSE_UNAUTHORIZED);
        }
        ws.send(Message::Text(ready.to_string()))
            .await
            .map_err(|_| CLOSE_PROTOCOL)?;
        keys
    };
    if let Ok(mut limiter) = shared.limiter.lock() {
        limiter.succeed(source, &device.id);
    }
    let (send_key, receive_key) = keys.session(&gateway_nonce, &phone_nonce);
    Ok(Session {
        device: device.id,
        profile: device.profile,
        send_key,
        receive_key,
    })
}

/// The pairing connection's only exchange under the pair-secret keys: the
/// phone sends `pairComplete {phoneSecret}` as its first frame, the gateway
/// stores it, activates the device and acknowledges. Both sides then switch
/// to keys derived from `pair_secret || phone_secret`, with sequence numbers
/// starting again at 0.
async fn complete_pairing(
    shared: &Arc<Shared>,
    ws: &mut WebSocketStream<TcpStream>,
    keys: &DeviceKeys,
    gateway_nonce: &[u8; NONCE_LEN],
    phone_nonce: &[u8; NONCE_LEN],
    device: &str,
    ip: String,
) -> Result<[u8; crypto::KEY_LEN], u16> {
    let (send_key, receive_key) = keys.session(gateway_nonce, phone_nonce);
    let frame = loop {
        match ws.next().await {
            Some(Ok(Message::Binary(frame))) => break frame,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return Err(CLOSE_PROTOCOL),
        }
    };
    let plaintext = Channel::new(&receive_key)
        .open(&frame)
        .map_err(|_| CLOSE_PROTOCOL)?;
    let request: Value = serde_json::from_slice(&plaintext).map_err(|_| CLOSE_PROTOCOL)?;
    let phone_secret = request["params"]["phoneSecret"]
        .as_str()
        .and_then(crypto::from_b64url::<{ crypto::KEY_LEN }>)
        .filter(|_| request["op"] == "pairComplete")
        .ok_or(CLOSE_PROTOCOL)?;
    let committed = {
        let store = shared.store.clone();
        let device = device.to_string();
        tokio::task::spawn_blocking(move || store.complete_pairing(&device, &phone_secret, ip))
            .await
    };
    if !matches!(committed, Ok(Ok(()))) {
        return Err(CLOSE_UNAUTHORIZED);
    }
    let ack =
        json!({"id":request.get("id").cloned().unwrap_or(Value::Null),"result":{"paired":true}});
    let sealed = Channel::new(&send_key)
        .seal(ack.to_string().as_bytes())
        .map_err(|_| CLOSE_PROTOCOL)?;
    ws.send(Message::Binary(sealed))
        .await
        .map_err(|_| CLOSE_PROTOCOL)?;
    Ok(phone_secret)
}

type Outbox = mpsc::Sender<Value>;

async fn run_session(
    shared: &Arc<Shared>,
    ws: WebSocketStream<TcpStream>,
    session: Session,
    mut closed: watch::Receiver<bool>,
    outbox: Outbox,
    mut queue: mpsc::Receiver<Value>,
) -> &'static str {
    let (mut sink, mut source) = ws.split();
    let mut sender = Channel::new(&session.send_key);
    let mut receiver = Channel::new(&session.receive_key);
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        ping.tick().await;
        loop {
            let message = tokio::select! {
                next = queue.recv() => match next {
                    Some(Value::Null) | None => break,
                    Some(value) => match serde_json::to_vec(&value).map_err(|e| e.to_string()).and_then(|b| sender.seal(&b)) {
                        Ok(frame) => Message::Binary(frame),
                        Err(_) => break,
                    },
                },
                _ = ping.tick() => Message::Ping(Vec::new()),
            };
            if sink.send(message).await.is_err() {
                return sink;
            }
        }
        sink
    });
    let requests = Arc::new(Semaphore::new(REQUESTS_IN_FLIGHT));
    let mut subscriptions: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    let outcome: (&'static str, u16) = loop {
        let frame = tokio::select! {
            _ = closed.changed() => break ("revoked", CLOSE_REVOKED),
            frame = source.next() => frame,
        };
        let bytes = match frame {
            Some(Ok(Message::Binary(bytes))) => bytes,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Close(_))) | None => break ("closed", 1000),
            Some(Ok(_)) => break ("plaintext frame", CLOSE_PROTOCOL),
            Some(Err(_)) => break ("transport error", CLOSE_PROTOCOL),
        };
        let Ok(plaintext) = receiver.open(&bytes) else {
            break ("rejected frame", CLOSE_PROTOCOL);
        };
        let Ok(request) = serde_json::from_slice::<Value>(&plaintext) else {
            break ("invalid request", CLOSE_PROTOCOL);
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let op = request["op"].as_str().unwrap_or("").to_string();
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let action = match policy::check(&op, &params) {
            Ok(action) => action,
            Err(refusal) => {
                let _ = outbox.send(refused(&id, refusal)).await;
                continue;
            }
        };
        match action {
            Action::Subscribe {
                conversation_id,
                after,
            } => {
                if let Some(old) = subscriptions.remove(&conversation_id) {
                    old.abort();
                }
                subscriptions.retain(|_, task| !task.is_finished());
                if subscriptions.len() >= MAX_SUBSCRIPTIONS {
                    let _ = outbox
                        .send(refused(
                            &id,
                            Refusal {
                                code: "limit",
                                message: format!("At most {MAX_SUBSCRIPTIONS} subscriptions"),
                            },
                        ))
                        .await;
                    continue;
                }
                let task = tokio::spawn(follow(
                    shared.panel.clone(),
                    session.profile.clone(),
                    conversation_id.clone(),
                    after,
                    outbox.clone(),
                ));
                subscriptions.insert(conversation_id.clone(), task);
                let _ = outbox
                    .send(json!({"id":id,"result":{"subscribed":conversation_id,"after":after}}))
                    .await;
            }
            Action::Unsubscribe { conversation_id } => {
                let found = subscriptions.remove(&conversation_id);
                if let Some(task) = &found {
                    task.abort();
                }
                let _ = outbox
                    .send(json!({"id":id,"result":{"unsubscribed":found.is_some()}}))
                    .await;
            }
            action => {
                let Ok(permit) = requests.clone().acquire_owned().await else {
                    break ("shutdown", 1001);
                };
                let shared = shared.clone();
                let session = session.clone();
                let outbox = outbox.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let reply = match perform(&shared, &session, action).await {
                        Ok(result) => json!({"id":id,"result":result}),
                        Err(refusal) => refused(&id, refusal),
                    };
                    let _ = outbox.send(reply).await;
                });
            }
        }
    };
    for task in subscriptions.values() {
        task.abort();
    }
    let _ = outbox.send(Value::Null).await;
    drop(outbox);
    if let Ok(mut sink) = writer.await {
        let _ = sink
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(outcome.1),
                reason: outcome.0.into(),
            })))
            .await;
        let _ = sink.close().await;
    }
    outcome.0
}

fn refused(id: &Value, refusal: Refusal) -> Value {
    json!({"id":id,"error":{"code":refusal.code,"message":refusal.message}})
}

fn panel_error(message: String) -> Refusal {
    Refusal {
        code: "panel",
        message,
    }
}

async fn perform(
    shared: &Arc<Shared>,
    session: &Session,
    action: Action,
) -> Result<Value, Refusal> {
    let profile = session.profile.as_str();
    match action {
        Action::Panel { operation, params } => shared
            .panel
            .call(operation, params, profile)
            .await
            .map_err(panel_error),
        Action::SetOptions {
            conversation_id,
            options,
        } => {
            let row = conversation_row(shared, profile, &conversation_id).await?;
            let agent = row["agentId"].as_str().unwrap_or("");
            let full = row["fullAccess"] == true;
            let current =
                policy::effective_rank(agent, row["options"]["toolAccess"].as_str(), full);
            let requested = policy::effective_rank(
                agent,
                options.get("toolAccess").and_then(Value::as_str),
                full,
            );
            if requested > current {
                return Err(Refusal {
                    code: "denied",
                    message: "The phone cannot raise a conversation's tool access".into(),
                });
            }
            shared
                .panel
                .call(
                    "conversationSetOptions",
                    json!({"conversationId":conversation_id,"options":options}),
                    profile,
                )
                .await
                .map_err(panel_error)
        }
        Action::ScheduleToggle { id, enabled } => {
            let list = shared
                .panel
                .call("scheduleList", json!({}), profile)
                .await
                .map_err(panel_error)?;
            let exists = list["schedules"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s["id"] == id.as_str()));
            if !exists {
                return Err(Refusal {
                    code: "notFound",
                    message: "Schedule not found; the phone cannot create schedules".into(),
                });
            }
            shared
                .panel
                .call("scheduleSave", json!({"id":id,"enabled":enabled}), profile)
                .await
                .map_err(panel_error)
        }
        Action::PushRegister { relay_token } => {
            let store = shared.store.clone();
            let device = session.device.clone();
            let refused =
                tokio::task::spawn_blocking(move || store.register_token(&device, &relay_token))
                    .await
                    .map_err(|_| "Device store unavailable".to_string())
                    .and_then(|r| r)
                    .map_err(|message| Refusal {
                        code: "store",
                        message,
                    })?;
            Ok(match refused {
                Some(reason) => json!({"registered":false,"reason":reason}),
                None => json!({"registered":true}),
            })
        }
        Action::PushUnregister => {
            set_relay_token(shared, &session.device, None).await?;
            Ok(json!({"registered":false}))
        }
        Action::Hello => Ok(json!({
            "computerName": shared.computer_name,
            "gatewayVersion": env!("CARGO_PKG_VERSION"),
        })),
        Action::Screenshot { conversation_id } => {
            {
                let mut last = shared.screenshots.lock().map_err(|_| Refusal {
                    code: "panel",
                    message: "Screenshot state unavailable".into(),
                })?;
                let now = Instant::now();
                if last
                    .get(&conversation_id)
                    .is_some_and(|at| now.duration_since(*at) < SCREENSHOT_INTERVAL)
                {
                    return Err(Refusal {
                        code: "rateLimited",
                        message: "One screenshot per conversation every 2 seconds".into(),
                    });
                }
                last.retain(|_, at| now.duration_since(*at) < SCREENSHOT_INTERVAL);
                last.insert(conversation_id.clone(), now);
            }
            // Only a conversation of this device's profile can be captured.
            conversation_row(shared, profile, &conversation_id).await?;
            capture(profile, &conversation_id)
                .await
                .map_err(|message| Refusal {
                    code: "screenshot",
                    message,
                })
        }
        Action::Subscribe { .. } | Action::Unsubscribe { .. } => unreachable!("handled inline"),
    }
}

async fn conversation_row(
    shared: &Arc<Shared>,
    profile: &str,
    conversation_id: &str,
) -> Result<Value, Refusal> {
    let list = shared
        .panel
        .call("conversationList", json!({}), profile)
        .await
        .map_err(panel_error)?;
    list["conversations"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["id"] == conversation_id))
        .cloned()
        .ok_or_else(|| Refusal {
            code: "notFound",
            message: "Conversation was not found in this profile".into(),
        })
}

async fn set_relay_token(
    shared: &Arc<Shared>,
    device: &str,
    token: Option<String>,
) -> Result<(), Refusal> {
    let store = shared.store.clone();
    let device = device.to_string();
    tokio::task::spawn_blocking(move || {
        store.update(|r| {
            r.device_mut(&device)
                .ok_or("Device was revoked")?
                .relay_token = token;
            Ok(())
        })
    })
    .await
    .map_err(|_| "Device store unavailable".to_string())
    .and_then(|r| r)
    .map_err(|message| Refusal {
        code: "store",
        message,
    })
}

fn busy(phase: &Value) -> bool {
    matches!(
        phase.as_str(),
        Some("starting" | "running" | "waitingInput" | "stopping")
    )
}

/// Poll one conversation's journal for a subscribed phone, at the panel's
/// own cadence, from the phone's cursor. Events are sent in order and the
/// cursor travels with them, so a reconnect resumes without gaps or repeats.
async fn follow(
    panel: Panel,
    profile: String,
    conversation: String,
    mut cursor: u64,
    outbox: Outbox,
) {
    let mut last_summary = Value::Null;
    let mut reported = false;
    loop {
        let read = panel
            .poll(
                "conversationRead",
                json!({"conversationId":conversation,"after":cursor}),
                &profile,
            )
            .await;
        match read {
            Ok(reply) => {
                reported = false;
                let events = reply["journal"]["events"].clone();
                let next = reply["journal"]["cursor"].as_u64().unwrap_or(cursor);
                let more = reply["journal"]["more"] == true;
                let summary = reply["conversation"].clone();
                let has_events = events.as_array().is_some_and(|e| !e.is_empty());
                if has_events || summary != last_summary {
                    let frame = json!({"event":"journal","conversationId":conversation,"conversation":summary,"events":events,"cursor":next,"more":more,"queuedMessages":reply["queuedMessages"]});
                    if outbox.send(frame).await.is_err() {
                        return;
                    }
                }
                cursor = next;
                let wait = if busy(&summary["phase"]) { 600 } else { 1800 };
                last_summary = summary;
                if more {
                    continue;
                }
                tokio::time::sleep(Duration::from_millis(wait)).await;
            }
            Err(error) => {
                if error.contains("not found") {
                    let _ = outbox
                        .send(json!({"event":"subscriptionEnded","conversationId":conversation,"reason":error}))
                        .await;
                    return;
                }
                if !reported {
                    reported = true;
                    let frame = json!({"event":"subscriptionError","conversationId":conversation,"error":error});
                    if outbox.send(frame).await.is_err() {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

/// Capture the conversation's own browser session (`panel-<id>`, the session
/// the panel service gives its agent) as a downscaled JPEG. A session without
/// a running daemon is reported, never started: a phone never opens a browser.
async fn capture(profile: &str, conversation: &str) -> Result<Value, String> {
    let session = format!("panel-{conversation}");
    if cfg!(test) || !crate::connection::daemon_ready(&session) {
        return Err("This conversation has no open browser session".into());
    }
    let executable = std::env::current_exe().map_err(|_| "Cannot locate chrome-use")?;
    let directory = tempfile::tempdir().map_err(|_| "Cannot create a screenshot folder")?;
    let path = directory.path().join("screenshot.png");
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("screenshot")
        .arg(&path)
        .env("AGENT_BROWSER_BROWSER", profile)
        .env("AGENT_BROWSER_SESSION", &session)
        .env("CHROME_USE_NO_UPDATE_CHECK", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(20), command.status())
        .await
        .map_err(|_| "The screenshot did not finish within 20 seconds")?
        .map_err(|_| "chrome-use could not run")?;
    if !status.success() {
        return Err("The browser session could not take a screenshot".into());
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| "The screenshot file is missing")?;
    tokio::task::spawn_blocking(move || jpeg(&bytes))
        .await
        .map_err(|_| "Screenshot encoding failed".to_string())?
}

pub(crate) fn jpeg(bytes: &[u8]) -> Result<Value, String> {
    use base64::Engine;
    let mut image = image::load_from_memory(bytes).map_err(|_| "Unreadable screenshot")?;
    if image.width() > SCREENSHOT_WIDTH {
        let height =
            (image.height() as u64 * SCREENSHOT_WIDTH as u64 / image.width() as u64).max(1) as u32;
        image = image.resize_exact(
            SCREENSHOT_WIDTH,
            height,
            image::imageops::FilterType::Triangle,
        );
    }
    let rgb = image.to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70)
        .encode_image(&rgb)
        .map_err(|_| "Screenshot encoding failed")?;
    Ok(
        json!({"mediaType":"image/jpeg","width":rgb.width(),"height":rgb.height(),"data":base64::engine::general_purpose::STANDARD.encode(out)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_after_free_failures_and_clears_on_success() {
        let mut limiter = Limiter::default();
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let now = Instant::now();
        for _ in 0..FREE_FAILURES - 1 {
            limiter.fail(ip, Some("d"), now);
        }
        assert!(!limiter.source_blocked(ip, now));
        limiter.fail(ip, Some("d"), now);
        assert!(limiter.source_blocked(ip, now));
        assert!(limiter.device_blocked("d", now));
        assert!(!limiter.source_blocked(ip, now + Duration::from_secs(2)));
        for _ in 0..40 {
            limiter.fail(ip, None, now);
        }
        assert!(limiter.source_blocked(ip, now + Duration::from_secs(299)));
        assert!(!limiter.source_blocked(ip, now + MAX_BACKOFF + Duration::from_secs(1)));
        limiter.succeed(ip, "d");
        assert!(!limiter.source_blocked(ip, now));
        assert!(!limiter.device_blocked("d", now));
    }

    #[test]
    fn screenshots_are_downscaled_jpeg() {
        let image = image::DynamicImage::new_rgb8(2160, 1200);
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let shot = jpeg(&png.into_inner()).unwrap();
        assert_eq!(shot["mediaType"], "image/jpeg");
        assert_eq!(shot["width"], 1080);
        assert_eq!(shot["height"], 600);
    }
}
