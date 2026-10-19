//! Push notifications through the stateless push Worker. The gateway watches
//! every conversation and schedule of the profiles that have a device with a
//! relay token, and sends an end-to-end encrypted notice when one needs the
//! user, ends a turn, or finishes a scheduled run.

use super::crypto::{self, DeviceKeys};
use super::gateway::Shared;
use super::store::{self, Device, State, Store};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DEFAULT_URL: &str = "https://push.chrome-use.leeguoo.com";
const POLL: Duration = Duration::from_secs(2);
const DEBOUNCE: Duration = Duration::from_secs(10);
const TITLE_CHARS: usize = 80;
const BODY_CHARS: usize = 120;
const MAX_PAYLOAD: usize = 2048;

/// The Worker URL: `--push-url`, else `CHROME_USE_PUSH_URL`, else the default.
pub fn url(flag: Option<String>) -> String {
    flag.or_else(|| std::env::var("CHROME_USE_PUSH_URL").ok())
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_URL.into())
        .trim_end_matches('/')
        .to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub conversation_id: String,
    pub kind: &'static str,
    pub title: String,
    pub body: String,
    pub high: bool,
}

impl Notice {
    pub fn plaintext(&self) -> Vec<u8> {
        json!({"v":1,"conversationId":self.conversation_id,"kind":self.kind,"title":self.title,"body":self.body})
            .to_string()
            .into_bytes()
    }
}

fn clip(text: &str, chars: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= chars {
        return line.to_string();
    }
    let mut out: String = line.chars().take(chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn busy(phase: &str) -> bool {
    matches!(phase, "starting" | "running" | "waitingInput" | "stopping")
}

/// What one profile looked like at the previous poll.
#[derive(Default)]
pub struct Observed {
    /// None until the first successful poll: nothing is announced for state
    /// that existed before the gateway started watching.
    phases: Option<HashMap<String, String>>,
    finished: Option<HashMap<String, Option<u64>>>,
}

impl Observed {
    /// Notices for the changes between the previous and this conversation list.
    pub fn conversations(&mut self, list: &Value) -> Vec<Notice> {
        let rows = list["conversations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut now = HashMap::new();
        let mut notices = Vec::new();
        for row in &rows {
            let (Some(id), Some(phase)) = (row["id"].as_str(), row["phase"].as_str()) else {
                continue;
            };
            now.insert(id.to_string(), phase.to_string());
            let Some(before) = &self.phases else { continue };
            let previous = before.get(id).map(String::as_str).unwrap_or("idle");
            let title = clip(row["title"].as_str().unwrap_or("chrome-use"), TITLE_CHARS);
            if phase == "waitingInput" && previous != "waitingInput" {
                notices.push(Notice {
                    conversation_id: id.into(),
                    kind: "needsInput",
                    title,
                    body: "Waiting for your approval".into(),
                    high: true,
                });
            } else if busy(previous) && !busy(phase) && row["scheduleId"].is_null() {
                let preview = row["lastTurn"]["preview"]
                    .as_str()
                    .map(|p| clip(p, BODY_CHARS));
                let body = preview.filter(|p| !p.is_empty()).unwrap_or_else(|| {
                    match phase {
                        "idle" => "Finished",
                        "interrupted" => "Stopped",
                        "failed" => "Failed",
                        _ => "Needs a look",
                    }
                    .into()
                });
                notices.push(Notice {
                    conversation_id: id.into(),
                    kind: "turnEnded",
                    title,
                    body,
                    high: false,
                });
            }
        }
        self.phases = Some(now);
        notices
    }

    /// Notices for scheduled runs that finished since the previous list.
    pub fn schedules(&mut self, list: &Value) -> Vec<Notice> {
        let rows = list["schedules"].as_array().cloned().unwrap_or_default();
        let mut now = HashMap::new();
        let mut notices = Vec::new();
        for row in &rows {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            let finished = row["finishedAt"].as_u64();
            now.insert(id.to_string(), finished);
            let Some(before) = &self.finished else {
                continue;
            };
            if finished.is_none() || before.get(id).copied().flatten() == finished {
                continue;
            }
            let result = row["lastResult"].as_str().unwrap_or("");
            let done = result == "completed";
            notices.push(Notice {
                conversation_id: row["lastChat"].as_str().unwrap_or("").into(),
                kind: if done {
                    "scheduleDone"
                } else {
                    "scheduleFailed"
                },
                title: clip(
                    row["name"].as_str().unwrap_or("Scheduled task"),
                    TITLE_CHARS,
                ),
                body: if done {
                    "Scheduled run finished".into()
                } else {
                    clip(result, BODY_CHARS)
                },
                high: false,
            });
        }
        self.finished = Some(now);
        notices
    }
}

/// At most one notice per conversation per window; a notice that arrives
/// inside the window replaces the waiting one and goes out when it closes.
#[derive(Default)]
pub struct Debouncer {
    sent: HashMap<String, Instant>,
    waiting: HashMap<String, Notice>,
}

impl Debouncer {
    pub fn offer(&mut self, notice: Notice) {
        let key = if notice.conversation_id.is_empty() {
            notice.title.clone()
        } else {
            notice.conversation_id.clone()
        };
        // A waiting "needs you" is not replaced by a lesser notice.
        if self
            .waiting
            .get(&key)
            .is_some_and(|w| w.high && !notice.high)
        {
            return;
        }
        self.waiting.insert(key, notice);
    }
    pub fn due(&mut self, now: Instant) -> Vec<Notice> {
        let mut out = Vec::new();
        let keys: Vec<String> = self.waiting.keys().cloned().collect();
        for key in keys {
            if self
                .sent
                .get(&key)
                .is_some_and(|at| now.duration_since(*at) < DEBOUNCE)
            {
                continue;
            }
            if let Some(notice) = self.waiting.remove(&key) {
                self.sent.insert(key, now);
                out.push(notice);
            }
        }
        self.sent.retain(|_, at| now.duration_since(*at) < DEBOUNCE);
        out
    }
}

pub enum Outcome {
    Sent,
    /// The token is dead (410, or 401 invalid/expired): drop it.
    DropToken(String),
    RetryAfter(Duration),
    Failed(String),
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

/// Encrypt one notice for one device and post it to the Worker.
pub async fn deliver(
    client: &reqwest::Client,
    url: &str,
    keys: &DeviceKeys,
    relay_token: &str,
    notice: &Notice,
) -> Outcome {
    let nonce = match crypto::random::<12>() {
        Ok(n) => n,
        Err(e) => return Outcome::Failed(e),
    };
    let payload = match crypto::seal_push(&keys.push, &nonce, &notice.plaintext()) {
        Ok(p) if p.len() <= MAX_PAYLOAD => p,
        Ok(_) => return Outcome::Failed("Push payload exceeds 2 KiB".into()),
        Err(e) => return Outcome::Failed(e),
    };
    let mut body = json!({"relayToken":relay_token,"payload":payload,"priority":if notice.high {"high"} else {"normal"}});
    if !notice.conversation_id.is_empty() {
        body["collapseId"] = json!(notice.conversation_id);
    }
    let response = match client
        .post(format!("{url}/v1/push"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(format!("push relay unreachable: {e}")),
    };
    let status = response.status().as_u16();
    let reply: Value = response.json().await.unwrap_or(Value::Null);
    match status {
        200 => Outcome::Sent,
        410 => Outcome::DropToken("unregistered".into()),
        401 => Outcome::DropToken(reply["status"].as_str().unwrap_or("invalid_token").into()),
        429 => Outcome::RetryAfter(Duration::from_secs(
            reply["retryAfter"].as_u64().unwrap_or(60).clamp(1, 3600),
        )),
        _ => Outcome::Failed(format!(
            "push relay answered {status} {}",
            reply["status"].as_str().unwrap_or("")
        )),
    }
}

async fn drop_token(store: &Store, device: &str, token: &str) {
    let store = store.clone();
    let device = device.to_string();
    let token = token.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        store.update(|r| {
            if let Some(d) = r.device_mut(&device) {
                // Only the token that failed; the phone may have sent a new one.
                if d.relay_token.as_deref() == Some(token.as_str()) {
                    d.relay_token = None;
                }
            }
            Ok(())
        })
    })
    .await;
}

/// Send one notice to every given device, handling dead tokens and rate limits.
pub async fn fan_out(
    client: &reqwest::Client,
    url: &str,
    store: &Store,
    devices: &[Device],
    notice: &Notice,
    paused: &mut HashMap<String, Instant>,
) {
    for device in devices {
        let Some(token) = &device.relay_token else {
            continue;
        };
        if paused
            .get(&device.id)
            .is_some_and(|until| *until > Instant::now())
        {
            continue;
        }
        let Ok(secret) = store.secret(&device.id) else {
            continue;
        };
        let keys = DeviceKeys::derive(&secret);
        match deliver(client, url, &keys, token, notice).await {
            Outcome::Sent => {}
            Outcome::DropToken(reason) => {
                eprintln!("mobile: push token of device {} dropped ({reason}); the app registers again on its next connection", device.id);
                drop_token(store, &device.id, token).await;
            }
            Outcome::RetryAfter(wait) => {
                paused.insert(device.id.clone(), Instant::now() + wait);
            }
            Outcome::Failed(error) => {
                eprintln!("mobile: push to device {} failed: {error}", device.id)
            }
        }
    }
}

pub(crate) async fn watch(shared: Arc<Shared>, url: String) {
    let client = client();
    let mut observed: HashMap<String, Observed> = HashMap::new();
    let mut debouncers: HashMap<String, Debouncer> = HashMap::new();
    let mut paused: HashMap<String, Instant> = HashMap::new();
    loop {
        tokio::time::sleep(POLL).await;
        let store = shared.store.clone();
        let Ok(Ok(registry)) = tokio::task::spawn_blocking(move || store.read()).await else {
            continue;
        };
        let mut by_profile: HashMap<String, Vec<Device>> = HashMap::new();
        for device in registry.devices {
            // A `--profile` gateway serves, and notifies for, that profile only.
            let in_scope = shared.profile.as_ref().is_none_or(|p| p == &device.profile);
            if in_scope && device.state == State::Active && device.relay_token.is_some() {
                by_profile
                    .entry(device.profile.clone())
                    .or_default()
                    .push(device);
            }
        }
        let watched: HashSet<&String> = by_profile.keys().collect();
        observed.retain(|p, _| watched.contains(p));
        debouncers.retain(|p, _| watched.contains(p));
        for (profile, devices) in &by_profile {
            let state = observed.entry(profile.clone()).or_default();
            let mut notices = Vec::new();
            if let Ok(list) = shared
                .panel
                .poll("conversationList", json!({}), profile)
                .await
            {
                notices.extend(state.conversations(&list));
            }
            if let Ok(list) = shared.panel.poll("scheduleList", json!({}), profile).await {
                notices.extend(state.schedules(&list));
            }
            let debouncer = debouncers.entry(profile.clone()).or_default();
            for notice in notices {
                debouncer.offer(notice);
            }
            for notice in debouncer.due(Instant::now()) {
                fan_out(&client, &url, &shared.store, devices, &notice, &mut paused).await;
            }
        }
    }
}

/// `mobile push-test <device>`.
pub async fn test(store: &Store, device_id: &str, url: &str) -> Result<(), String> {
    let registry = store.read()?;
    let device = registry
        .device(device_id)
        .ok_or("No paired device has that id")?;
    let token = device.relay_token.as_deref().ok_or(
        "This device has not registered for notifications yet; open the app while it is connected",
    )?;
    let keys = DeviceKeys::derive(&store.secret(device_id)?);
    let notice = Notice {
        conversation_id: String::new(),
        kind: "test",
        title: "chrome-use".into(),
        body: format!("Test notification ({})", store::now_ms() / 1000),
        high: false,
    };
    match deliver(&client(), url, &keys, token, &notice).await {
        Outcome::Sent => Ok(()),
        Outcome::DropToken(reason) => {
            drop_token(store, device_id, token).await;
            Err(format!("The push relay refused the token ({reason}); it was removed. Reconnect the app to register again"))
        }
        Outcome::RetryAfter(wait) => Err(format!(
            "The push relay is rate limiting this device; retry in {} s",
            wait.as_secs()
        )),
        Outcome::Failed(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e";

    fn list(phase: &str, preview: Option<&str>) -> Value {
        json!({"conversations":[{"id":A,"title":"Weekly report\nsecond line","phase":phase,"scheduleId":null,"lastTurn":preview.map(|p| json!({"preview":p}))}]})
    }

    #[test]
    fn transitions_become_notices_after_the_baseline() {
        let mut seen = Observed::default();
        assert!(seen.conversations(&list("waitingInput", None)).is_empty());
        assert!(seen.conversations(&list("running", None)).is_empty());
        let needs = seen.conversations(&list("waitingInput", None));
        assert_eq!(needs.len(), 1);
        assert_eq!(needs[0].kind, "needsInput");
        assert!(needs[0].high);
        assert_eq!(needs[0].title, "Weekly report");
        let ended = seen.conversations(&list("idle", Some("Sent the summary to Ann")));
        assert_eq!(ended[0].kind, "turnEnded");
        assert_eq!(ended[0].body, "Sent the summary to Ann");
        assert!(seen.conversations(&list("idle", None)).is_empty());
        // A scheduled conversation ends with a schedule notice instead.
        let mut scheduled = list("running", None);
        scheduled["conversations"][0]["scheduleId"] = json!(A);
        seen.conversations(&scheduled);
        scheduled["conversations"][0]["phase"] = json!("idle");
        assert!(seen.conversations(&scheduled).is_empty());
    }

    #[test]
    fn finished_schedules_are_announced_once() {
        let mut seen = Observed::default();
        let rows = |finished: Option<u64>, result: &str| json!({"schedules":[{"id":A,"name":"Daily","finishedAt":finished,"lastResult":result,"lastChat":A}]});
        assert!(seen.schedules(&rows(Some(1), "completed")).is_empty());
        assert!(seen.schedules(&rows(Some(1), "completed")).is_empty());
        let done = seen.schedules(&rows(Some(2), "completed"));
        assert_eq!(done[0].kind, "scheduleDone");
        let failed = seen.schedules(&rows(Some(3), &format!("failed: {}", "x".repeat(300))));
        assert_eq!(failed[0].kind, "scheduleFailed");
        assert!(failed[0].body.chars().count() <= BODY_CHARS);
        assert!(seen.schedules(&rows(None, "starting")).is_empty());
    }

    #[test]
    fn debounce_coalesces_per_conversation() {
        let mut debouncer = Debouncer::default();
        let notice = |kind: &'static str, high: bool| Notice {
            conversation_id: A.into(),
            kind,
            title: "t".into(),
            body: "b".into(),
            high,
        };
        let start = Instant::now();
        debouncer.offer(notice("needsInput", true));
        assert_eq!(debouncer.due(start).len(), 1);
        debouncer.offer(notice("turnEnded", false));
        assert!(debouncer.due(start + Duration::from_secs(5)).is_empty());
        debouncer.offer(notice("needsInput", true));
        debouncer.offer(notice("turnEnded", false));
        let later = debouncer.due(start + DEBOUNCE);
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].kind, "needsInput");
    }

    #[test]
    fn push_payload_decrypts_with_the_device_push_key() {
        let keys = DeviceKeys::derive(&[3u8; 32]);
        let notice = Notice {
            conversation_id: A.into(),
            kind: "needsInput",
            title: clip(&"t".repeat(500), TITLE_CHARS),
            body: clip(&"b".repeat(500), BODY_CHARS),
            high: true,
        };
        let payload = crypto::seal_push(&keys.push, &[0u8; 12], &notice.plaintext()).unwrap();
        assert!(payload.len() <= MAX_PAYLOAD);
        let plain: Value =
            serde_json::from_slice(&crypto::open_push(&keys.push, &payload).unwrap()).unwrap();
        assert_eq!(plain["v"], 1);
        assert_eq!(plain["kind"], "needsInput");
        assert_eq!(plain["conversationId"], A);
    }

    #[test]
    fn push_url_precedence() {
        assert_eq!(url(Some("https://x.test/".into())), "https://x.test");
    }
}
