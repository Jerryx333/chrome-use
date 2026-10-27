//! `teach start/stop/status`: record what the user does by hand in the
//! session's tab, for `crate::teach` to turn into a script or adapter.
//!
//! The recorder (`extensions/ab-connect/teach-recorder.js`) runs in an
//! isolated world created for this recording under a random name, so the page
//! cannot see its listeners or variables. It reports each action through a
//! `Runtime.addBinding` binding added **only to that world**
//! (`executionContextName`); the page's main world never gets it. Events that
//! could not be sent (no binding yet, or a relay that refuses bindings) stay
//! queued in the world and are drained with `Runtime.evaluate` in that world.
//!
//! Nothing runs before the user's page script: Chrome only re-adds a binding
//! to new documents with `Runtime.enable`, which pages can detect, so the
//! recorder is installed again after each main-frame navigation. An action in
//! the first moments of a new page, before the recorder is back, can be
//! missed; the review step shows what was captured.
//!
//! `teach stop` removes the listeners and the binding. Nothing is left in the
//! page or in the relay's replay list.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::actions::DaemonState;
use super::cdp::client::CdpClient;

/// A recording in progress.
pub struct Recording {
    pub name: String,
    pub start_url: String,
    session_id: String,
    events: Arc<Mutex<Vec<Value>>>,
    bound: Arc<Mutex<bool>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    started: std::time::Instant,
}

impl Drop for Recording {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn random_name(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}

async fn main_frame(client: &CdpClient, sid: &str) -> Result<String, String> {
    let tree = client
        .send_command("Page.getFrameTree", None, Some(sid))
        .await?;
    tree.pointer("/frameTree/frame/id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "teach: the tab has no main frame".to_string())
}

/// Create (or find) the recording's world in the main frame, add the binding
/// to it and start the recorder. Returns the world's context and whether the
/// binding is there.
async fn install(
    client: &CdpClient,
    sid: &str,
    world: &str,
    binding: &str,
) -> Result<(i64, bool), String> {
    let frame = main_frame(client, sid).await?;
    let made = client
        .send_command(
            "Page.createIsolatedWorld",
            Some(json!({"frameId": frame, "worldName": world, "grantUniveralAccess": false})),
            Some(sid),
        )
        .await?;
    let ctx = made
        .get("executionContextId")
        .and_then(Value::as_i64)
        .ok_or("teach: Chrome did not create the recording's isolated world")?;
    // Only contexts named `world` get the binding; the page's main world is
    // unnamed. A relay that refuses it (an older extension in stealth mode)
    // leaves the recorder to be drained by polling.
    let bound = client
        .send_command(
            "Runtime.addBinding",
            Some(json!({"name": binding, "executionContextName": world})),
            Some(sid),
        )
        .await
        .is_ok();
    let emit = if bound {
        format!(
            "p => {{ const b = globalThis[{b}]; if (typeof b !== 'function') return false; b(JSON.stringify(p)); }}",
            b = serde_json::to_string(binding).unwrap_or_default()
        )
    } else {
        "() => false".to_string()
    };
    let source = format!(
        "{}\n;(() => {{ const r = globalThis.__cuTeachRecorder; return !!(r && r.install({emit}) && r.active()); }})()",
        crate::teach::RECORDER_JS
    );
    let out = client
        .send_command(
            "Runtime.evaluate",
            Some(json!({"expression": source, "contextId": ctx, "returnByValue": true})),
            Some(sid),
        )
        .await?;
    if out.get("exceptionDetails").is_some()
        || out.pointer("/result/value").and_then(Value::as_bool) != Some(true)
    {
        return Err("teach: the recorder did not start in the page's isolated world".into());
    }
    Ok((ctx, bound))
}

async fn drain(client: &CdpClient, sid: &str, ctx: i64) -> Vec<Value> {
    let out = client
        .send_command(
            "Runtime.evaluate",
            Some(json!({
                "expression": "(() => { const r = globalThis.__cuTeachRecorder; return r ? r.drain() : []; })()",
                "contextId": ctx,
                "returnByValue": true,
            })),
            Some(sid),
        )
        .await;
    out.ok()
        .and_then(|v| v.pointer("/result/value").cloned())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

async fn teardown(client: &CdpClient, sid: &str, world: &str, binding: &str) {
    if let Ok(frame) = main_frame(client, sid).await {
        if let Ok(made) = client
            .send_command(
                "Page.createIsolatedWorld",
                Some(json!({"frameId": frame, "worldName": world, "grantUniveralAccess": false})),
                Some(sid),
            )
            .await
        {
            if let Some(ctx) = made.get("executionContextId").and_then(Value::as_i64) {
                let _ = client
                    .send_command(
                        "Runtime.evaluate",
                        Some(json!({
                            "expression": "(() => { const r = globalThis.__cuTeachRecorder; return r ? r.uninstall() : true; })()",
                            "contextId": ctx,
                            "returnByValue": true,
                        })),
                        Some(sid),
                    )
                    .await;
            }
        }
    }
    let _ = client
        .send_command(
            "Runtime.removeBinding",
            Some(json!({"name": binding})),
            Some(sid),
        )
        .await;
}

fn push(events: &Mutex<Vec<Value>>, mut ev: Value, doc: u64) {
    if let Some(o) = ev.as_object_mut() {
        o.insert("doc".into(), json!(doc));
        o.entry("ts").or_insert(json!(now_ms()));
    }
    let mut list = events.lock().unwrap();
    if list.len() < 20_000 {
        list.push(ev);
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    client: Arc<CdpClient>,
    sid: String,
    world: String,
    binding: String,
    mut frame: String,
    mut ctx: Option<i64>,
    events: Arc<Mutex<Vec<Value>>>,
    bound: Arc<Mutex<bool>>,
    mut rx: tokio::sync::broadcast::Receiver<super::cdp::types::CdpEvent>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut doc: u64 = 0;
    let mut nav_reason = String::new();
    let mut poll = tokio::time::interval(Duration::from_millis(400));
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = poll.tick() => {
                if *bound.lock().unwrap() { continue; }
                if let Some(c) = ctx {
                    for ev in drain(&client, &sid, c).await {
                        push(&events, ev, doc);
                    }
                }
            }
            got = rx.recv() => {
                let ev = match got {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                if ev.session_id.as_deref() != Some(sid.as_str()) {
                    continue;
                }
                let p = &ev.params;
                match ev.method.as_str() {
                    "Runtime.bindingCalled" if p["name"].as_str() == Some(binding.as_str()) => {
                        if let Some(parsed) = p["payload"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()) {
                            push(&events, parsed, doc);
                        }
                    }
                    "Page.frameRequestedNavigation" if p["frameId"].as_str() == Some(frame.as_str()) => {
                        nav_reason = p["reason"].as_str().unwrap_or("").to_string();
                        // The old document is still there: collect what it queued.
                        if let Some(c) = ctx {
                            for ev in drain(&client, &sid, c).await {
                                push(&events, ev, doc);
                            }
                        }
                    }
                    "Page.frameNavigated" if p.pointer("/frame/parentId").and_then(Value::as_str).is_none_or(str::is_empty) => {
                        if let Some(id) = p.pointer("/frame/id").and_then(Value::as_str) {
                            frame = id.to_string();
                        }
                        let url = p.pointer("/frame/url").and_then(Value::as_str).unwrap_or("").to_string();
                        push(&events, json!({"t": "nav", "url": url, "reason": std::mem::take(&mut nav_reason)}), doc);
                        doc += 1;
                        ctx = None;
                        for attempt in 0..4 {
                            match install(&client, &sid, &world, &binding).await {
                                Ok((c, b)) => {
                                    ctx = Some(c);
                                    *bound.lock().unwrap() = b;
                                    break;
                                }
                                Err(_) if attempt < 3 => tokio::time::sleep(Duration::from_millis(150)).await,
                                Err(_) => {}
                            }
                        }
                    }
                    "Page.navigatedWithinDocument" if p["frameId"].as_str() == Some(frame.as_str()) => {
                        push(&events, json!({"t": "nav", "url": p["url"], "sameDocument": true}), doc);
                    }
                    "Inspector.detached" | "Target.detachedFromTarget" => break,
                    _ => {}
                }
            }
        }
    }
    if let Some(c) = ctx {
        for ev in drain(&client, &sid, c).await {
            push(&events, ev, doc);
        }
    }
    teardown(&client, &sid, &world, &binding).await;
}

/// `teach_start {name?}`
pub async fn handle_start(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    if state.teach.is_some() {
        return Err("teach: already recording in this session; run `chrome-use teach stop` (or `teach cancel`) first".into());
    }
    let mgr = state
        .browser
        .as_ref()
        .ok_or("Browser not launched; open the page you want to teach first")?;
    let sid = mgr.active_session_id()?.to_string();
    let client = mgr.client.clone();
    let start_url = mgr.get_url().await.unwrap_or_default();
    if !(start_url.starts_with("http://")
        || start_url.starts_with("https://")
        || start_url.starts_with("file://"))
    {
        return Err(format!(
            "teach: open the page the flow starts on first (the tab is on {})",
            if start_url.is_empty() {
                "nothing"
            } else {
                &start_url
            }
        ));
    }
    let name = cmd
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("flow-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let world = random_name("w");
    let binding = random_name("b");
    // Subscribe before installing, so no event between the two is lost.
    let rx = client.subscribe();
    let frame = main_frame(&client, &sid).await?;
    let (ctx, bound_now) = install(&client, &sid, &world, &binding).await?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let bound = Arc::new(Mutex::new(bound_now));
    let (stop_tx, stop_rx) = oneshot::channel();
    let task = tokio::spawn(run(
        client,
        sid.clone(),
        world,
        binding,
        frame,
        Some(ctx),
        events.clone(),
        bound.clone(),
        rx,
        stop_rx,
    ));
    state.teach = Some(Recording {
        name: name.clone(),
        start_url: start_url.clone(),
        session_id: sid,
        events,
        bound,
        stop: Some(stop_tx),
        task: Some(task),
        started: std::time::Instant::now(),
    });
    let mut out = json!({
        "recording": true,
        "name": name,
        "startUrl": start_url,
        "channel": if bound_now { "binding" } else { "poll" },
    });
    if !bound_now {
        out["warning"] = json!("the relay refused an isolated-world binding (update the chrome-use extension); events are polled instead, and an action right before a page change can be missed");
    }
    Ok(out)
}

/// `teach_status`
pub fn handle_status(state: &DaemonState) -> Result<Value, String> {
    Ok(match &state.teach {
        None => json!({"recording": false}),
        Some(r) => json!({
            "recording": true,
            "name": r.name,
            "startUrl": r.start_url,
            "events": r.events.lock().unwrap().iter().filter(|e| e["t"] != "nav").count(),
            "channel": if *r.bound.lock().unwrap() { "binding" } else { "poll" },
            "elapsedMs": r.started.elapsed().as_millis() as u64,
        }),
    })
}

/// `teach_stop {discard?}`: end the recording and hand back what it saw.
pub async fn handle_stop(cmd: &Value, state: &mut DaemonState) -> Result<Value, String> {
    let mut rec = state.teach.take().ok_or(
        "teach: nothing is being recorded in this session; start with `chrome-use teach start`",
    )?;
    if let Some(stop) = rec.stop.take() {
        let _ = stop.send(());
    }
    let mut finished = true;
    if let Some(mut task) = rec.task.take() {
        finished = tokio::time::timeout(Duration::from_secs(8), &mut task)
            .await
            .is_ok();
        if !finished {
            task.abort();
        }
    }
    let events = std::mem::take(&mut *rec.events.lock().unwrap());
    let mut recording = json!({"startUrl": rec.start_url, "events": events});
    crate::teach::redact_recording(&mut recording);
    let discard = cmd.get("discard").and_then(Value::as_bool) == Some(true);
    let mut out = json!({
        "stopped": true,
        "name": rec.name,
        "sessionTab": rec.session_id,
        "durationMs": rec.started.elapsed().as_millis() as u64,
        "recording": if discard { Value::Null } else { recording },
        "discarded": discard,
    });
    if !finished {
        out["warning"] = json!("the recorder did not confirm its cleanup within 8s; reload the page to be sure no listener is left");
    }
    Ok(out)
}
