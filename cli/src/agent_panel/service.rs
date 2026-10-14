//! A profile-owned agent service, independent of Chrome's native-message port.
//! The loopback protocol is private native IPC, not a page or browser endpoint.
//! Each endpoint is authenticated by a capability kept in a mode-0600 file.

use super::backend::{self, Command as BackendCommand, Notice, Setup};
use super::config_transaction::{atomic_write, private_open, private_tree};
use super::conversation::{Conversation, Message, PageContext, Phase, Store};
use super::options::ModelOptions;
use super::DiscoveryContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};

const MAX_FRAME: usize = 768 * 1024;
static BINARY_HASH: LazyLock<Result<String, String>> = LazyLock::new(|| {
    let path =
        std::env::current_exe().map_err(|_| "Cannot identify the agent-service executable")?;
    let mut file = File::open(path).map_err(|_| "Cannot inspect the agent-service executable")?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file
            .read(&mut bytes)
            .map_err(|_| "Cannot fingerprint the agent-service executable")?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(hex::encode(hash.finalize()))
});

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Endpoint {
    schema_version: u32,
    profile: String,
    address: SocketAddr,
    capability: String,
    binary_hash: String,
    pid: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    capability: String,
    request_id: String,
    operation: String,
    params: Value,
}

fn profile_directory(root: &Path, profile: &str) -> Result<PathBuf, String> {
    if !root.is_absolute()
        || profile.is_empty()
        || profile.len() > 80
        || !profile
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(
            "Agent service requires an absolute data directory and a bound profile identity".into(),
        );
    }
    for directory in [
        root.to_path_buf(),
        root.join("profiles"),
        root.join("profiles").join(profile),
    ] {
        private_tree(&directory)?;
    }
    Ok(root.join("profiles").join(profile))
}

pub(super) fn data_root(context: &DiscoveryContext) -> Result<PathBuf, String> {
    if let Some(directory) = context
        .environment
        .get("CHROME_USE_RELAY_DIR")
        .filter(|s| !s.is_empty())
    {
        let directory = PathBuf::from(directory);
        if !directory.is_absolute() {
            return Err("CHROME_USE_RELAY_DIR must be absolute".into());
        }
        Ok(directory.join("agent-panel"))
    } else {
        Ok(context.home.join(".chrome-use/agent-panel"))
    }
}

fn endpoint(directory: &Path, profile: &str) -> Result<Endpoint, String> {
    let path = directory.join("service.json");
    let meta =
        std::fs::symlink_metadata(&path).map_err(|_| "Agent service endpoint is unavailable")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 8192 {
        return Err("Invalid agent service endpoint file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != unsafe { libc::geteuid() } || meta.permissions().mode() & 0o077 != 0 {
            return Err("Agent service endpoint is not private".into());
        }
    }
    let endpoint: Endpoint = serde_json::from_slice(
        &std::fs::read(path).map_err(|_| "Cannot read agent service endpoint")?,
    )
    .map_err(|_| "Invalid agent service endpoint")?;
    if endpoint.schema_version != 1
        || endpoint.profile != profile
        || !endpoint.address.ip().is_loopback()
        || endpoint.capability.len() != 64
    {
        return Err("Agent service endpoint identity does not match this profile".into());
    }
    Ok(endpoint)
}

/// How long one service operation may take. A CLI model check can need a cold
/// start (OpenCode starts a private server); a delete waits for the agent to
/// exit and then closes the conversation's browser session.
fn operation_seconds(operation: &str) -> u64 {
    match operation {
        "agentInspect" => 30,
        "skillImport" => 40,
        "conversationDelete" => 45,
        _ => 10,
    }
}

fn exchange(endpoint: &Endpoint, operation: &str, params: Value) -> Result<Value, String> {
    let mut stream = TcpStream::connect_timeout(&endpoint.address, Duration::from_millis(750))
        .map_err(|_| "Agent service did not answer")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(operation_seconds(operation) + 2)))
        .map_err(|_| "Cannot set agent service timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| "Cannot set agent service timeout")?;
    let request = Request {
        capability: endpoint.capability.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: operation.into(),
        params,
    };
    let raw = serde_json::to_vec(&request).map_err(|_| "Cannot encode agent service request")?;
    if raw.len() > MAX_FRAME {
        return Err("Agent service request is too large".into());
    }
    stream
        .write_all(&(raw.len() as u32).to_le_bytes())
        .and_then(|_| stream.write_all(&raw))
        .map_err(|_| "Agent service request could not be written; outcome is unconfirmed")?;
    let mut length = [0u8; 4];
    stream
        .read_exact(&mut length)
        .map_err(|_| "Agent service reply is unconfirmed; no request was replayed")?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err("Agent service reply exceeds its size limit".into());
    }
    let mut raw = vec![0u8; length];
    stream
        .read_exact(&mut raw)
        .map_err(|_| "Agent service reply is incomplete; no request was replayed")?;
    let reply: Value = serde_json::from_slice(&raw).map_err(|_| "Invalid agent service reply")?;
    if reply["requestId"] != request.request_id {
        return Err("Agent service replied to a different request".into());
    }
    if let Some(error) = reply["error"].as_str() {
        return Err(error.into());
    }
    reply
        .get("result")
        .cloned()
        .ok_or_else(|| "Agent service reply has no result".into())
}

/// Reuse only a health-confirmed service of the exact binary. A different
/// build's active conversations are never killed or silently rebound. Idle
/// builds are replaced only after endpoint removal and OS lock release.
fn ensure(directory: &Path, root: &Path, profile: &str) -> Result<Endpoint, String> {
    let binary_hash = BINARY_HASH.as_ref().map_err(Clone::clone)?;
    if let Ok(existing) = endpoint(directory, profile) {
        if let Ok(health) = exchange(&existing, "health", json!({})) {
            if health["binaryHash"] == *binary_hash {
                return Ok(existing);
            }
        }
    }
    let bootstrap = private_open(&directory.join("bootstrap.lock"), true)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while bootstrap.try_lock().is_err() {
        if Instant::now() > deadline {
            return Err("Another agent-service startup is still in progress".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if let Ok(existing) = endpoint(directory, profile) {
        if let Ok(health) = exchange(&existing, "health", json!({})) {
            if health["binaryHash"] == *binary_hash {
                return Ok(existing);
            }
            if health["active"].as_u64() != Some(0) {
                return Err("Another agent-panel build is running for this profile; stop its conversations before updating".into());
            }
            // The old service rechecks busy state under its own lock. Wait for
            // both endpoint removal and OS ownership release, never a PID.
            exchange(&existing, "serviceStop", json!({}))?;
            loop {
                let ownership = private_open(&directory.join("service.lock"), true)?;
                if !directory.join("service.json").exists() && ownership.try_lock().is_ok() {
                    break;
                }
                if Instant::now() > deadline {
                    return Err("Previous agent service shutdown is unconfirmed; no replacement was started".into());
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    }
    // Probe the real OS ownership lock, without trusting a stale PID file.
    let ownership = private_open(&directory.join("service.lock"), true)?;
    ownership.try_lock().map_err(|_| {
        "The profile's agent service owns its lock but does not respond; it was not restarted"
    })?;
    drop(ownership);
    let executable =
        std::env::current_exe().map_err(|_| "Cannot locate agent-service executable")?;
    let arguments: Vec<std::ffi::OsString> = vec![
        "__panel-service".into(),
        "--profile".into(),
        profile.into(),
        "--data-dir".into(),
        root.as_os_str().to_owned(),
    ];
    // Under Chrome every descendant inherits its process quarantine, so files
    // an agent writes are quarantined. Bun-built CLIs extract and dlopen their
    // native libraries, which Gatekeeper then blocks. launchd starts the
    // service outside Chrome's process tree.
    #[cfg(target_os = "macos")]
    let launched = launchd_start(directory, &executable, &arguments).is_ok();
    #[cfg(not(target_os = "macos"))]
    let launched = false;
    let mut child = if launched {
        None
    } else {
        Some(spawn_detached(&executable, &arguments)?)
    };
    while Instant::now() < deadline {
        if let Some(status) = match child.as_mut() {
            Some(child) => child
                .try_wait()
                .map_err(|_| "Cannot inspect agent service startup")?,
            None => None,
        } {
            return Err(format!("Agent service exited during startup ({status})"));
        }
        if let Ok(endpoint) = endpoint(directory, profile) {
            if endpoint.binary_hash == *binary_hash
                && exchange(&endpoint, "health", json!({})).is_ok()
            {
                return Ok(endpoint);
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Err("Agent-service startup has not been confirmed; its process was not restarted".into())
}

fn spawn_detached(
    executable: &Path,
    arguments: &[std::ffi::OsString],
) -> Result<std::process::Child, String> {
    let mut command = std::process::Command::new(executable);
    command
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200);
    }
    command
        .spawn()
        .map_err(|_| "Agent service could not be started".into())
}

/// Load a one-shot launchd job for the service. The label is per data
/// directory; a finished job stays loaded but idle, so it is booted out
/// before each start. The plist carries this process's environment, so it
/// lives only until launchd has read it.
#[cfg(target_os = "macos")]
fn launchd_start(
    directory: &Path,
    executable: &Path,
    arguments: &[std::ffi::OsString],
) -> Result<(), String> {
    let digest = Sha256::digest(directory.as_os_str().as_encoded_bytes());
    let label = format!(
        "com.chrome-use.panel-service.{}",
        &hex::encode(digest)[..16]
    );
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let launchctl = |args: &[&std::ffi::OsStr]| {
        std::process::Command::new("/bin/launchctl")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let plist = launchd_plist(&label, executable, arguments)?;
    let path = directory.join("service.plist");
    atomic_write(&path, plist.as_bytes(), None)?;
    // A quarantined writer quarantines the plist too, and launchd refuses
    // to load a quarantined job definition.
    {
        use std::os::unix::ffi::OsStrExt;
        let file = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Invalid agent-service plist path")?;
        unsafe { libc::removexattr(file.as_ptr(), c"com.apple.quarantine".as_ptr(), 0) };
    }
    let target = format!("{domain}/{label}");
    let _ = launchctl(&["bootout".as_ref(), target.as_ref()]);
    // bootout returns before launchd forgets the label; retry briefly.
    let mut loaded = false;
    for _ in 0..20 {
        if launchctl(&["bootstrap".as_ref(), domain.as_ref(), path.as_os_str()]) {
            loaded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(&path);
    if loaded {
        Ok(())
    } else {
        Err("launchd did not accept the agent service".into())
    }
}

#[cfg(target_os = "macos")]
fn launchd_plist(
    label: &str,
    executable: &Path,
    arguments: &[std::ffi::OsString],
) -> Result<String, String> {
    fn text(value: &std::ffi::OsStr) -> Option<String> {
        let value = value.to_str()?;
        if value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
        {
            return None;
        }
        Some(
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
        )
    }
    let mut program = String::new();
    for argument in
        std::iter::once(executable.as_os_str()).chain(arguments.iter().map(|a| a.as_os_str()))
    {
        let argument = text(argument).ok_or("Agent-service arguments are not plist text")?;
        program.push_str(&format!("<string>{argument}</string>"));
    }
    let mut environment = String::new();
    for (key, value) in std::env::vars_os() {
        if let (Some(key), Some(value)) = (text(&key), text(&value)) {
            environment.push_str(&format!("<key>{key}</key><string>{value}</string>"));
        }
    }
    let directory = std::env::current_dir()
        .ok()
        .and_then(|d| text(d.as_os_str()))
        .unwrap_or_else(|| "/".into());
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array>{program}</array>
<key>EnvironmentVariables</key><dict>{environment}</dict>
<key>WorkingDirectory</key><string>{directory}</string>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><false/>
<key>ProcessType</key><string>Interactive</string>
<key>AbandonProcessGroup</key><true/>
</dict>
</plist>
"#
    ))
}

pub(super) fn call(
    operation: &str,
    params: &Value,
    profile: Option<&str>,
) -> Result<Value, String> {
    let profile = profile.ok_or("Conversation operations require a bound Chrome profile")?;
    let context = DiscoveryContext::current()?;
    let root = data_root(&context)?;
    let directory = profile_directory(&root, profile)?;
    // Explicit stop and read operations may reach an older schema-1 service.
    // This lets the user stop its work without dispatching a new turn.
    if [
        "serviceStop",
        "conversationList",
        "conversationRead",
        "conversationMessageStatus",
        "conversationStop",
    ]
    .contains(&operation)
    {
        if let Ok(existing) = endpoint(&directory, profile) {
            if exchange(&existing, "health", json!({})).is_ok() {
                return exchange(&existing, operation, params.clone());
            }
        }
    }
    let endpoint = ensure(&directory, &root, profile)?;
    exchange(
        &endpoint,
        if operation == "serviceHealth" {
            "health"
        } else {
            operation
        },
        params.clone(),
    )
}

struct Job {
    generation: u64,
    full_access: bool,
    options: ModelOptions,
    retiring: bool,
    sender: mpsc::Sender<BackendCommand>,
}
struct Engine {
    store: Store,
    chats: HashMap<String, Conversation>,
    jobs: HashMap<String, Job>,
    requests: HashMap<(String, String), (Value, String, Value, bool)>,
    profile: String,
    stop: bool,
    schedules: std::collections::BTreeMap<String, super::schedules::Schedule>,
}

impl Engine {
    /// Answer every waiting approval for a full-access conversation exactly as
    /// the panel's "Allow once" would. Questions still go to the user.
    fn approve_pending(&mut self, id: &str) {
        let Some(sender) = self.jobs.get(id).map(|j| j.sender.clone()) else {
            return;
        };
        for ((chat, _), (request_id, method, original, sent)) in self.requests.iter_mut() {
            if chat != id || *sent {
                continue;
            }
            let Some(result) = automatic_approval(method, original) else {
                continue;
            };
            if validate_reply(method, original, &result).is_err() {
                continue;
            }
            if sender
                .try_send(BackendCommand::Reply {
                    id: request_id.clone(),
                    result,
                })
                .is_ok()
            {
                *sent = true;
            }
        }
    }
    fn chat(&mut self, id: &str) -> Result<&mut Conversation, String> {
        if !self.chats.contains_key(id) {
            if self.chats.len() >= 32 {
                let victim = self
                    .chats
                    .iter()
                    .filter(|(key, c)| !self.jobs.contains_key(*key) && !c.phase.busy())
                    .min_by_key(|(_, c)| c.updated)
                    .map(|(key, _)| key.clone());
                if let Some(victim) = victim {
                    self.chats.remove(&victim);
                }
            }
            let chat = self.store.load(id)?;
            self.chats.insert(id.into(), chat);
        }
        self.chats
            .get_mut(id)
            .ok_or_else(|| "Conversation was not found".into())
    }
    fn persist(&mut self, id: &str) -> Result<(), String> {
        let chat = self.chats.get(id).ok_or("Conversation was not found")?;
        self.store.save(chat)
    }
    fn record(&mut self, id: &str, kind: &str, data: Value) -> Result<(), String> {
        let chat = self.chats.get_mut(id).ok_or("Conversation was not found")?;
        self.store.event(chat, kind, data)?;
        if kind != "textDelta" {
            self.store.save(chat)?;
        }
        Ok(())
    }
    fn notice(
        &mut self,
        id: &str,
        generation: u64,
        mut notice: Notice,
    ) -> Result<Option<Message>, String> {
        if self
            .jobs
            .get(id)
            .is_some_and(|job| job.generation == generation && job.retiring)
        {
            if notice.kind == "exited" {
                self.jobs.remove(id);
                self.requests.retain(|(chat, _), _| chat != id);
                return Ok(None);
            }
            if notice.kind != "processUnconfirmed" {
                return Ok(None);
            }
        }
        if self.chat(id)?.generation != generation {
            return Ok(None);
        }
        if self
            .jobs
            .get(id)
            .is_some_and(|job| job.generation != generation)
        {
            return Ok(None);
        }
        if let Some(message) = self.chat(id)?.active.as_ref() {
            notice.data["messageId"] = message.id.clone().into();
        }
        // Claude confirms a permission round trip by returning the matching
        // tool_use_id. A stdin write alone never resolves the request.
        if notice.kind == "tool" && notice.data["completed"] == true {
            if let Some(tool_id) = notice.data["item"]["tool_use_id"].as_str() {
                let resolved: Vec<_> = self
                    .requests
                    .iter()
                    .filter(|((chat, _), (_, method, params, sent))| {
                        chat == id
                            && *sent
                            && method == "can_use_tool"
                            && params["tool_use_id"] == tool_id
                    })
                    .map(|(key, (request_id, _, _, _))| (key.clone(), request_id.clone()))
                    .collect();
                for (key, request_id) in resolved {
                    self.requests.remove(&key);
                    self.record(id, "requestResolved", json!({"requestId":request_id}))?;
                }
            }
        }
        match notice.kind {
            "ready" => {
                let chat = self.chat(id)?;
                chat.session_id = notice.data["sessionId"].as_str().map(String::from);
                chat.model = notice.data["model"].as_str().map(String::from);
            }
            "running" | "textDelta" | "tool" => {
                let waiting = self.requests.keys().any(|(chat, _)| chat == id);
                let chat = self.chat(id)?;
                if chat.active.is_some() && chat.phase != Phase::Stopping {
                    chat.phase = if waiting {
                        Phase::WaitingInput
                    } else {
                        Phase::Running
                    };
                }
            }
            "request" => {
                let request_id = notice.data["requestId"].clone();
                let key =
                    serde_json::to_string(&request_id).map_err(|_| "Invalid backend request id")?;
                if key.len() > 1024 || self.requests.len() >= 64 {
                    return Err("Backend request capacity reached".into());
                }
                self.requests.insert(
                    (id.into(), key),
                    (
                        request_id,
                        notice.data["method"].as_str().unwrap_or("").into(),
                        notice.data["params"].clone(),
                        false,
                    ),
                );
                if self.chat(id)?.phase != Phase::Stopping {
                    self.chat(id)?.phase = Phase::WaitingInput;
                }
                if self.chat(id)?.full_access {
                    self.approve_pending(id);
                }
            }
            "requestResolved" => {
                let key = serde_json::to_string(&notice.data["requestId"])
                    .map_err(|_| "Invalid backend request id")?;
                self.requests.remove(&(id.into(), key));
            }
            "title" => {
                if let Some(title) = notice.data["title"].as_str() {
                    self.chat(id)?.title = title.chars().take(80).collect();
                }
            }
            "terminal" => {
                let status = notice.data["status"]
                    .as_str()
                    .ok_or("Missing backend terminal status")?;
                self.chat(id)?.terminal(
                    generation,
                    status,
                    notice.data["error"].as_str().map(String::from),
                )?;
                self.requests.retain(|(chat, _), _| chat != id);
            }
            "exited" => {
                self.jobs.remove(id);
                self.requests.retain(|(chat, _), _| chat != id);
                let chat = self.chat(id)?;
                if chat.active.is_some() {
                    let status = if chat.phase == Phase::Stopping {
                        "interrupted"
                    } else {
                        "failed"
                    };
                    chat.terminal(
                        generation,
                        status,
                        Some(
                            notice.data["error"]
                                .as_str()
                                .unwrap_or("Backend process ended before the turn completed")
                                .into(),
                        ),
                    )?;
                }
            }
            "processUnconfirmed" => {
                self.jobs.remove(id);
                self.requests.retain(|(chat, _), _| chat != id);
                let chat = self.chat(id)?;
                chat.phase = Phase::RecoveryRequired;
                chat.queue_paused = true;
                chat.last_error = Some(
                    notice.data["error"]
                        .as_str()
                        .unwrap_or("Backend process state is unconfirmed")
                        .into(),
                );
            }
            _ => {}
        }
        self.record(id, notice.kind, notice.data)?;
        if ["terminal", "exited"].contains(&notice.kind) {
            let next = self.chat(id)?.begin_next();
            if next.is_some() {
                self.persist(id)?;
            }
            Ok(next)
        } else {
            Ok(None)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Create {
    agent_id: String,
    #[serde(default)]
    options: ModelOptions,
    model: Option<String>,
    executable: Option<PathBuf>,
    #[serde(default)]
    full_access: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetOptions {
    conversation_id: String,
    options: ModelOptions,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetAccess {
    conversation_id: String,
    full_access: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Identify {
    conversation_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Send {
    conversation_id: String,
    message_id: String,
    text: String,
    #[serde(default)]
    page: Option<PageContext>,
    #[serde(default)]
    attachments: Vec<super::attachments::Attachment>,
    skill_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MessageStatus {
    conversation_id: String,
    message_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueueRemove {
    conversation_id: String,
    message_id: String,
    expected_text: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueueIdentify {
    conversation_id: String,
    message_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueueEdit {
    conversation_id: String,
    message_id: String,
    expected_text: String,
    replacement_id: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Events {
    conversation_id: String,
    #[serde(default)]
    after: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reply {
    conversation_id: String,
    request_id: Value,
    result: Value,
}

async fn dispatch(
    engine: &Arc<Mutex<Engine>>,
    operation: &str,
    params: &Value,
) -> Result<Value, String> {
    if engine.lock().await.stop && operation != "health" {
        return Err("Agent service is stopping; reconnect after its exit is confirmed".into());
    }
    match operation {
        "scheduleList"
        | "scheduleSave"
        | "scheduleDelete"
        | "scheduleRun"
        | "agentScheduleList"
        | "agentScheduleSave"
        | "agentScheduleDelete"
        | "agentScheduleRun" => schedule_dispatch(engine, operation, params).await,

        "agentInspect" => {
            let root = engine.lock().await.store.directory.clone();
            super::inspection::inspect(params, &root).await
        }
        "health" => {
            if params != &json!({}) {
                return Err("health takes no parameters".into());
            }
            let e = engine.lock().await;
            Ok(
                json!({"schemaVersion":1,"profile":e.profile,"binaryHash":BINARY_HASH.as_ref().map_err(Clone::clone)?,"pid":std::process::id(),"active":e.chats.values().filter(|c|c.phase.busy()).count()}),
            )
        }
        "serviceStop" => {
            if params != &json!({}) {
                return Err("serviceStop takes no parameters".into());
            }
            let mut e = engine.lock().await;
            if e.chats.values().any(|c| c.phase.busy()) {
                return Err("Stop active conversations before stopping the service".into());
            }
            let senders: Vec<_> = e.jobs.values().map(|j| j.sender.clone()).collect();
            for sender in senders {
                sender
                    .try_send(BackendCommand::Shutdown)
                    .map_err(|_| "Backend shutdown could not be dispatched")?;
            }
            e.stop = true;
            Ok(json!({"schemaVersion":1,"service":{"stopRequested":true}}))
        }
        "conversationList" => {
            if params != &json!({}) {
                return Err("conversationList takes no parameters".into());
            }
            let e = engine.lock().await;
            Ok(json!({"schemaVersion":1,"conversations":e.store.list()}))
        }

        "agentSkillList" | "agentSkillRead" | "agentSkillSave" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct AgentSkillRequest {
                caller_conversation: String,
                id: Option<String>,
                path: Option<String>,
                name: Option<String>,
                content: Option<String>,
            }
            let p: AgentSkillRequest = serde_json::from_value(params.clone())
                .map_err(|_| "Invalid skill tool arguments")?;
            let mut e = engine.lock().await;
            let caller = e.chat(&p.caller_conversation)?.clone();
            if operation == "agentSkillList" {
                return Ok(
                    json!({"schemaVersion":1,"skills":super::skill_library::catalog(&e.store.directory,&caller.agent_id)?,"selectedSkill":caller.active.as_ref().and_then(|m|m.skill.as_ref()).map(|s|json!({"id":s.id,"name":s.name}))}),
                );
            }
            if operation == "agentSkillSave" {
                if caller.schedule_id.is_some() {
                    return Err(
                        "Scheduled conversations cannot modify skill instructions for future runs"
                            .into(),
                    );
                }
                super::skill_library::save(
                    &e.store.directory,
                    p.name.as_deref().ok_or("Skill name is required")?,
                    p.content.as_deref().ok_or("Skill content is required")?,
                )?;
                return Ok(
                    json!({"schemaVersion":1,"skills":super::skill_library::catalog(&e.store.directory,&caller.agent_id)?}),
                );
            }
            if let Some(path) = p.path {
                let skill = caller
                    .active
                    .as_ref()
                    .and_then(|m| m.skill.as_ref())
                    .ok_or("No skill is selected for the active message")?;
                if p.id.as_deref().is_some_and(|id| id != skill.id) {
                    return Err(
                        "Asset reads must belong to the active message's selected skill".into(),
                    );
                }
                let workspace = e.store.directory.join(&caller.id).join("workspace");
                return Ok(
                    json!({"schemaVersion":1,"skill":{"id":skill.id,"name":skill.name,"path":path,"content":super::skill_library::read_asset(&workspace,skill,&path)?}}),
                );
            }
            let skill = if let Some(id) = p.id {
                super::skill_library::read_entry(&e.store.directory, &caller.agent_id, &id)?
            } else {
                caller
                    .active
                    .as_ref()
                    .and_then(|m| m.skill.clone())
                    .ok_or("No skill selected; list the catalog and specify an ID")?
            };
            Ok(json!({"schemaVersion":1,"skill":skill}))
        }
        "skillList" | "skillRead" | "skillSave" | "skillSetEnabled" | "skillImport" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct SkillRequest {
                agent_id: String,
                id: Option<String>,
                name: Option<String>,
                content: Option<String>,
                enabled: Option<bool>,
                kind: Option<String>,
                source: Option<String>,
            }
            let p: SkillRequest = serde_json::from_value(params.clone())
                .map_err(|_| "Skill operation requires agentId and entry details")?;
            if !super::CHAT_AGENTS.contains(&p.agent_id.as_str()) {
                return Err("Unknown panel agent".into());
            }
            let root = engine.lock().await.store.directory.clone();
            if operation == "skillImport" {
                super::skill_library::import(
                    &root,
                    p.kind.as_deref().ok_or("Import kind is required")?,
                    p.source.as_deref().ok_or("Import source is required")?,
                )
                .await?;
            }
            if operation == "skillSetEnabled" {
                super::skill_library::set_enabled(
                    &root,
                    &p.agent_id,
                    p.id.as_deref().ok_or("Skill ID is required")?,
                    p.enabled.ok_or("Enabled choice is required")?,
                )?;
            }
            if operation == "skillRead" {
                let skill = super::skill_library::read_entry(
                    &root,
                    &p.agent_id,
                    p.id.as_deref().ok_or("Skill ID is required")?,
                )?;
                return Ok(json!({"schemaVersion":1,"skill":skill}));
            }
            if operation == "skillSave" {
                super::skill_library::save(
                    &root,
                    p.name.as_deref().ok_or("Skill name is required")?,
                    p.content.as_deref().ok_or("Skill content is required")?,
                )?;
            }
            Ok(
                json!({"schemaVersion":1,"skills":super::skill_library::catalog(&root,&p.agent_id)?}),
            )
        }
        "conversationSkillSelect" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct SelectSkill {
                conversation_id: String,
                skill_id: String,
            }
            let p: SelectSkill = serde_json::from_value(params.clone())
                .map_err(|_| "Select a skill with conversationId and skillId")?;
            let mut e = engine.lock().await;
            let agent = e.chat(&p.conversation_id)?.agent_id.clone();
            let workspace = e.store.directory.join(&p.conversation_id).join("workspace");
            private_tree(&workspace)?;
            let skill = super::skill_library::snapshot(
                &e.store.directory,
                &agent,
                &p.skill_id,
                &workspace,
            )?;
            Ok(
                json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary(),"skill":{"id":skill.id,"name":skill.name,"catalogId":p.skill_id}}),
            )
        }
        "conversationAttachmentPut" | "conversationAttachmentRead" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct ImageRequest {
                conversation_id: String,
                data: Option<String>,
                attachment: Option<super::attachments::Attachment>,
            }
            let p: ImageRequest = serde_json::from_value(params.clone())
                .map_err(|_| "Image request needs conversationId and image data or attachment")?;
            let mut e = engine.lock().await;
            e.chat(&p.conversation_id)?;
            let workspace = e.store.directory.join(&p.conversation_id).join("workspace");
            private_tree(&workspace)?;
            let attachment = if operation == "conversationAttachmentPut" {
                super::attachments::save(
                    &workspace,
                    p.data.as_deref().ok_or("Image data is required")?,
                )?
            } else {
                p.attachment.ok_or("Attachment identity is required")?
            };
            let data = if operation == "conversationAttachmentRead" {
                Some(attachment.base64(&workspace)?)
            } else {
                None
            };
            Ok(
                json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary(),"attachment":attachment,"data":data}),
            )
        }
        "conversationCreate" => {
            let p: Create = serde_json::from_value(params.clone()).map_err(|_| {
                "conversationCreate requires an agent id and optional model/executable"
            })?;
            if p.executable.as_ref().is_some_and(|p| !p.is_absolute()) {
                return Err("A custom agent executable must be an absolute path".into());
            }
            let mut chat = Conversation::new(&p.agent_id, p.model, p.executable)?;
            p.options.validate(&p.agent_id)?;
            chat.options = p.options;
            chat.full_access = p.full_access;
            let mut e = engine.lock().await;
            e.store.create(&chat)?;
            let result = chat.summary();
            e.chats.insert(chat.id.clone(), chat);
            Ok(json!({"schemaVersion":1,"conversation":result}))
        }
        "conversationRead" => {
            let p: Events = serde_json::from_value(params.clone()).map_err(|_| {
                "conversationRead requires conversationId and optional after cursor"
            })?;
            let mut e = engine.lock().await;
            let summary = e.chat(&p.conversation_id)?.summary();
            let events = e.store.events(&p.conversation_id, p.after)?;
            let queued = e.chat(&p.conversation_id)?.queue_snapshot();
            Ok(
                json!({"schemaVersion":1,"conversation":summary,"journal":events,"queuedMessages":queued}),
            )
        }
        "conversationQueueMessageRead" => {
            let p: QueueIdentify = serde_json::from_value(params.clone())
                .map_err(|_| "Queue message read requires conversationId and messageId")?;
            let mut e = engine.lock().await;
            let chat = e.chat(&p.conversation_id)?;
            let message = chat
                .queue
                .iter()
                .find(|m| m.id == p.message_id)
                .ok_or("Message is no longer queued; read its current state")?;
            Ok(json!({"schemaVersion":1,"conversation":chat.summary(),"queuedMessage":message}))
        }
        "conversationQueueEdit" => {
            let p: QueueEdit = serde_json::from_value(params.clone()).map_err(|_| "Queue edit requires conversationId, messageId, expectedText, replacementId and text")?;
            let mut e = engine.lock().await;
            e.chat(&p.conversation_id)?.edit_queued(
                &p.message_id,
                &p.expected_text,
                Message {
                    id: p.replacement_id.clone(),
                    text: p.text.clone(),
                    page: None,
                    attachments: Vec::new(),
                    skill: None,
                },
            )?;
            e.record(
                &p.conversation_id,
                "queueEdited",
                json!({"messageId":p.message_id,"replacementId":p.replacement_id,"text":p.text}),
            )?;
            Ok(
                json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary(),"queueChange":{"messageId":p.message_id,"replacementId":p.replacement_id,"kind":"edited"}}),
            )
        }
        "conversationQueueRemove" => {
            let p: QueueRemove = serde_json::from_value(params.clone())
                .map_err(|_| "Queue removal requires conversationId, messageId and expectedText")?;
            let mut e = engine.lock().await;
            e.chat(&p.conversation_id)?
                .remove_queued(&p.message_id, &p.expected_text)?;
            e.record(
                &p.conversation_id,
                "queueRemoved",
                json!({"messageId":p.message_id}),
            )?;
            Ok(
                json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary(),"queueChange":{"messageId":p.message_id,"kind":"removed"}}),
            )
        }
        "conversationMessageStatus" => {
            let p: MessageStatus = serde_json::from_value(params.clone())
                .map_err(|_| "Message status requires conversationId and messageId")?;
            let mut e = engine.lock().await;
            let chat = e.chat(&p.conversation_id)?;
            let known = chat.accepted.contains_key(&p.message_id);
            let status = if chat.active.as_ref().is_some_and(|m| m.id == p.message_id) {
                "active"
            } else if chat.queue.iter().any(|m| m.id == p.message_id) {
                "queued"
            } else if known {
                "previouslyAccepted"
            } else {
                "unconfirmed"
            };
            Ok(
                json!({"schemaVersion":1,"conversation":chat.summary(),"message":{"id":p.message_id,"known":known,"status":status}}),
            )
        }
        "conversationSend" | "conversationSendDirect" => {
            let p: Send = serde_json::from_value(params.clone())
                .map_err(|_| "conversationSend requires conversationId, messageId and text")?;
            let mut e = engine.lock().await;
            let direct = operation == "conversationSendDirect";
            let interrupt = if direct && e.chat(&p.conversation_id)?.phase.busy() {
                Some(
                    e.jobs
                        .get(&p.conversation_id)
                        .ok_or("Backend is unavailable; stop cannot be confirmed")?
                        .sender
                        .clone(),
                )
            } else {
                None
            };
            if let Some(page) = &p.page {
                page.validate()?;
                let chat = e.chat(&p.conversation_id)?;
                if chat.site.is_none() {
                    chat.site = page_site(&page.url);
                }
            }
            if p.attachments.len() > super::attachments::MAX_IMAGES {
                return Err("A message supports up to five images".into());
            }
            let workspace = e.store.directory.join(&p.conversation_id).join("workspace");
            for image in &p.attachments {
                image.read(&workspace)?;
            }
            let message = Message {
                skill: p
                    .skill_id
                    .as_deref()
                    .map(|id| super::skill_library::load_snapshot(&workspace, id))
                    .transpose()?,
                attachments: p.attachments.clone(),
                id: p.message_id.clone(),
                text: p.text.clone(),
                page: p.page.clone(),
            };
            let added = if direct {
                e.chat(&p.conversation_id)?.accept_direct(message)?
            } else {
                e.chat(&p.conversation_id)?.accept(message)?
            };
            e.persist(&p.conversation_id)?;
            if added {
                e.record(
                    &p.conversation_id,
                    "user",
                    json!({"messageId":p.message_id,"text":p.text,"direct":direct,"page":p.page,"attachments":p.attachments,"skillId":p.skill_id}),
                )?;
            }
            if added {
                if let Some(sender) = interrupt {
                    e.chat(&p.conversation_id)?.phase = Phase::Stopping;
                    e.persist(&p.conversation_id)?;
                    if sender.try_send(BackendCommand::Interrupt).is_err() {
                        let chat = e.chat(&p.conversation_id)?;
                        chat.phase = Phase::RecoveryRequired;
                        chat.last_error =
                            Some("Direct message saved but stop dispatch is unconfirmed".into());
                        e.persist(&p.conversation_id)?;
                        return Err("Direct message saved but stop dispatch is unconfirmed; read its state without resending".into());
                    }
                }
            }
            let next = e.chat(&p.conversation_id)?.begin_next();
            e.persist(&p.conversation_id)?;
            drop(e);
            if let Some(message) = next {
                if let Err(error) = start(engine, &p.conversation_id, message).await {
                    let mut e = engine.lock().await;
                    let chat = e.chat(&p.conversation_id)?;
                    chat.phase = if direct {
                        Phase::RecoveryRequired
                    } else {
                        Phase::Failed
                    };
                    chat.queue_paused = true;
                    if !direct {
                        chat.active = None;
                    }
                    chat.last_error = Some(error);
                    e.persist(&p.conversation_id)?;
                }
            }
            let summary = engine.lock().await.chat(&p.conversation_id)?.summary();
            Ok(
                json!({"schemaVersion":1,"conversation":summary,"message":{"id":p.message_id,"accepted":added,"replayed":false}}),
            )
        }
        "conversationSetOptions" => {
            let p: SetOptions = serde_json::from_value(params.clone())
                .map_err(|_| "Conversation options require conversationId and options")?;
            let mut e = engine.lock().await;
            let chat = e.chat(&p.conversation_id)?;
            if chat.phase.busy() || chat.phase == Phase::RecoveryRequired {
                return Err(
                    "Stop the response and confirm its outcome before changing options".into(),
                );
            }
            p.options.validate(&chat.agent_id)?;
            chat.options = p.options;
            chat.updated = super::conversation::now();
            e.persist(&p.conversation_id)?;
            Ok(json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary()}))
        }
        "conversationSetAccess" => {
            let p: SetAccess = serde_json::from_value(params.clone())
                .map_err(|_| "conversationSetAccess requires conversationId and fullAccess")?;
            let mut e = engine.lock().await;
            e.chat(&p.conversation_id)?.full_access = p.full_access;
            e.persist(&p.conversation_id)?;
            // Requests already waiting are answered now; the agent process is
            // restarted with matching flags before its next turn.
            if p.full_access {
                e.approve_pending(&p.conversation_id);
            }
            Ok(json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary()}))
        }
        "conversationDelete" => {
            let p: Identify = serde_json::from_value(params.clone())
                .map_err(|_| "conversationDelete requires only conversationId")?;
            let id = p.conversation_id;
            let mut e = engine.lock().await;
            if e.chat(&id)?.phase.busy() {
                return Err("Stop the response before deleting this conversation".into());
            }
            // Files are removed only after the agent process is confirmed gone.
            if let Some(job) = e.jobs.get(&id) {
                job.sender
                    .try_send(BackendCommand::Shutdown)
                    .map_err(|_| "Agent shutdown was not dispatched; nothing was deleted")?;
                drop(e);
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                loop {
                    let locked = engine.lock().await;
                    if !locked.jobs.contains_key(&id) {
                        e = locked;
                        break;
                    }
                    drop(locked);
                    if tokio::time::Instant::now() > deadline {
                        return Err("The agent process has not exited yet; nothing was deleted. Try again shortly.".into());
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
            }
            // Real deletion also removes the CLI's own session history and the
            // conversation's browser session. Both run without the engine lock.
            let (agent_id, session_id, executable, workspace) = {
                let chat = e.chat(&id)?;
                (
                    chat.agent_id.clone(),
                    chat.session_id.clone(),
                    chat.executable.clone(),
                    e.store.directory.join(&id).join("workspace"),
                )
            };
            let profile = e.profile.clone();
            drop(e);
            let mut removed = Vec::new();
            let mut failed = Vec::new();
            let mut history_error = None;
            if let Some(session_id) = &session_id {
                if agent_id == "opencode" {
                    let executable = executable
                        .or_else(|| DiscoveryContext::current().ok()?.executable("opencode"));
                    match executable {
                        Some(executable) => {
                            match super::acp::delete_session(&executable, &workspace, session_id)
                                .await
                            {
                                Ok(()) => removed
                                    .push(PathBuf::from(format!("opencode session {session_id}"))),
                                Err(error) => history_error = Some(error),
                            }
                        }
                        None => {
                            history_error = Some(
                                "OpenCode executable unavailable; CLI history deletion unconfirmed"
                                    .into(),
                            )
                        }
                    }
                }
                match super::cli_session_files(&agent_id, session_id) {
                    Ok(paths) => {
                        for path in paths {
                            match super::remove_session_path(&path) {
                                Ok(()) => removed.push(path),
                                Err(_) => failed.push(path),
                            }
                        }
                    }
                    Err(error) => history_error = Some(error),
                }
            }
            let browser = close_browser_session(&profile, &format!("panel-{id}")).await;
            let mut e = engine.lock().await;
            if e.jobs.contains_key(&id) {
                return Err(
                    "The conversation started again while it was being deleted; delete it again"
                        .into(),
                );
            }
            e.chats.remove(&id);
            e.requests.retain(|(chat, _), _| chat != &id);
            e.store.delete(&id)?;
            Ok(
                json!({"schemaVersion":1,"deleted":{"id":id,"cliHistory":{"agentId":agent_id,"sessionId":session_id,"removed":removed,"failed":failed,"error":history_error},"browserSession":browser}}),
            )
        }
        "conversationStop" | "conversationResumeQueue" => {
            let p: Identify = serde_json::from_value(params.clone())
                .map_err(|_| "Conversation control requires only conversationId")?;
            let mut e = engine.lock().await;
            if operation == "conversationResumeQueue" {
                let chat = e.chat(&p.conversation_id)?;
                if chat.phase == Phase::RecoveryRequired {
                    return Err(
                        "The previous outcome is unconfirmed; the queue cannot resume".into(),
                    );
                }
                chat.queue_paused = false;
                if !chat.phase.busy() && chat.direct_message_id.is_some() {
                    chat.direct_ready = true;
                }
                let next = e.chat(&p.conversation_id)?.begin_next();
                e.persist(&p.conversation_id)?;
                let summary = e.chat(&p.conversation_id)?.summary();
                drop(e);
                if let Some(message) = next {
                    start(engine, &p.conversation_id, message).await?;
                }
                return Ok(json!({"schemaVersion":1,"conversation":summary}));
            }
            if e.chat(&p.conversation_id)?.phase.busy() {
                let sender = e
                    .jobs
                    .get(&p.conversation_id)
                    .ok_or("Backend process is unavailable; interruption is unconfirmed")?
                    .sender
                    .clone();
                e.chat(&p.conversation_id)?.phase = Phase::Stopping;
                e.persist(&p.conversation_id)?;
                sender.try_send(BackendCommand::Interrupt).map_err(|_| {
                    "Backend could not receive interruption; outcome is unconfirmed"
                })?;
            }
            Ok(json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary()}))
        }
        "conversationRespond" => {
            let p: Reply = serde_json::from_value(params.clone()).map_err(|_| {
                "Conversation response requires conversationId, requestId and result"
            })?;
            let mut e = engine.lock().await;
            let key = serde_json::to_string(&p.request_id).map_err(|_| "Invalid request id")?;
            let (id, method, original, sent) = e
                .requests
                .get(&(p.conversation_id.clone(), key.clone()))
                .ok_or("The backend request is stale or belongs to another conversation")?
                .clone();
            if sent {
                return Err(
                    "A response was already dispatched; wait for backend confirmation".into(),
                );
            }
            validate_reply(&method, &original, &p.result)?;
            let sender = e
                .jobs
                .get(&p.conversation_id)
                .ok_or("Backend is not available")?
                .sender
                .clone();
            sender
                .try_send(BackendCommand::Reply {
                    id,
                    result: p.result,
                })
                .map_err(|_| "Backend response could not be dispatched")?;
            e.requests
                .get_mut(&(p.conversation_id.clone(), key))
                .unwrap()
                .3 = true;
            Ok(
                json!({"schemaVersion":1,"conversation":e.chat(&p.conversation_id)?.summary(),"response":{"dispatched":true,"confirmed":false}}),
            )
        }
        _ => Err("Unsupported conversation service operation".into()),
    }
}

/// The host a conversation is listed under: the page's host without `www.`.
fn page_site(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(
        host.strip_prefix("www.")
            .unwrap_or(&host)
            .chars()
            .take(253)
            .collect(),
    )
}

fn automatic_approval(method: &str, params: &Value) -> Option<Value> {
    match method {
        "session/request_permission" => params["options"]
            .as_array()?
            .iter()
            .find(|option| option["kind"] == "allow_once")
            .map(|option| json!({"outcome":{"outcome":"selected","optionId":option["optionId"]}})),
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            Some(json!({"decision":"accept"}))
        }
        "item/permissions/requestApproval" => Some(
            json!({"permissions":params.get("permissions").cloned().unwrap_or(json!({})),"scope":"turn"}),
        ),
        "mcpServer/elicitation/request" => Some(json!({"action":"accept","content":{}})),
        "can_use_tool" if params["tool_name"] != "AskUserQuestion" => Some(
            json!({"behavior":"allow","updatedInput":params.get("input").cloned().unwrap_or(json!({}))}),
        ),
        _ => None,
    }
}

fn validate_reply(method: &str, original: &Value, result: &Value) -> Result<(), String> {
    if original["truncated"] == true {
        return Err("The request is too large to review through this surface".into());
    }
    let valid = match method {
        "session/request_permission" => {
            result.as_object().is_some_and(|o| o.len() == 1)
                && result["outcome"].as_object().is_some_and(|o| {
                    (o.len() == 1 && result["outcome"]["outcome"] == "cancelled")
                        || (o.len() == 2
                            && result["outcome"]["outcome"] == "selected"
                            && result["outcome"]["optionId"].is_string()
                            && original["options"].as_array().is_some_and(|options| {
                                options.iter().any(|option| {
                                    option["optionId"] == result["outcome"]["optionId"]
                                })
                            }))
                })
        }
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            result.as_object().is_some_and(|o| {
                o.len() == 1
                    && ["accept", "acceptForSession", "decline", "cancel"]
                        .contains(&result["decision"].as_str().unwrap_or(""))
            })
        }
        "item/tool/requestUserInput" => {
            result.as_object().is_some_and(|o| o.len() == 1)
                && result["answers"].as_object().is_some_and(|o| {
                    o.iter().all(|(key, v)| {
                        original["questions"]
                            .as_array()
                            .is_some_and(|q| q.iter().any(|q| q["id"] == *key))
                            && v["answers"]
                                .as_array()
                                .is_some_and(|a| a.iter().all(Value::is_string))
                    })
                })
        }
        "can_use_tool" => {
            ["allow", "deny"].contains(&result["behavior"].as_str().unwrap_or(""))
                && (result["behavior"] == "deny" || result.get("updatedInput").is_some())
        }
        "mcpServer/elicitation/request" => {
            let action = result["action"].as_str().unwrap_or("");
            let known_fields = result.as_object().is_some_and(|o| {
                o.keys()
                    .all(|k| ["action", "content"].contains(&k.as_str()))
            });
            let approval = original["serverName"] == "chrome_use"
                && original["mode"] == "form"
                && original["_meta"]["codex_approval_kind"] == "mcp_tool_call"
                && original["requestedSchema"]["type"] == "object"
                && original["requestedSchema"]["properties"]
                    .as_object()
                    .is_some_and(|o| o.is_empty())
                && original["requestedSchema"]
                    .get("required")
                    .is_none_or(|value| value.as_array().is_some_and(|a| a.is_empty()))
                && original["_meta"]["tool_params"].is_object();
            known_fields
                && match action {
                    "accept" => {
                        approval && result["content"].as_object().is_some_and(|o| o.is_empty())
                    }
                    "decline" | "cancel" => result.get("content").is_none_or(Value::is_null),
                    _ => false,
                }
        }
        "item/permissions/requestApproval" => {
            result.get("permissions").is_some()
                && ["turn", "session"].contains(&result["scope"].as_str().unwrap_or(""))
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("Response does not match the backend request's supported schema".into())
    }
}

fn start<'a>(
    engine: &'a Arc<Mutex<Engine>>,
    id: &'a str,
    message: Message,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + std::marker::Send + 'a>>
{
    Box::pin(async move {
        let mut e = engine.lock().await;
        let wanted = e.chat(id)?.clone();
        // A resumed CLI must have the requested conversation options. Wait for
        // the old process to exit before starting its replacement.
        if e.jobs.get(id).is_some_and(|job| {
            job.full_access != wanted.full_access || job.options != wanted.options
        }) {
            let sender = e.jobs.get(id).ok_or("Backend disappeared")?.sender.clone();
            sender
                .try_send(BackendCommand::Shutdown)
                .map_err(|_| "Backend shutdown was not dispatched")?;
            e.jobs.get_mut(id).ok_or("Backend disappeared")?.retiring = true;
            drop(e);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                let locked = engine.lock().await;
                if !locked.jobs.contains_key(id) {
                    e = locked;
                    if e.chat(id)?.phase == Phase::RecoveryRequired {
                        return Err("Previous backend exit is unconfirmed".into());
                    }
                    break;
                }
                drop(locked);
                if tokio::time::Instant::now() > deadline {
                    return Err(
                        "Backend shutdown has not been confirmed; no replacement started".into(),
                    );
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }
        if let Some(job) = e.jobs.get(id) {
            let sender = job.sender.clone();
            drop(e);
            return sender
                .send(BackendCommand::Start(message))
                .await
                .map_err(|_| {
                    "Backend process became unavailable before receiving the turn".into()
                });
        }
        if e.jobs.len() >= 4 {
            let idle = e
                .jobs
                .keys()
                .filter(|key| {
                    e.chats
                        .get(*key)
                        .is_some_and(|c| !c.phase.busy() && c.phase != Phase::RecoveryRequired)
                })
                .min_by_key(|key| e.chats.get(*key).map(|c| c.updated).unwrap_or(0))
                .cloned();
            if let Some(idle) = idle {
                let sender = e
                    .jobs
                    .get(&idle)
                    .ok_or("Idle backend disappeared")?
                    .sender
                    .clone();
                sender
                    .try_send(BackendCommand::Shutdown)
                    .map_err(|_| "Idle backend shutdown was not dispatched")?;
                drop(e);
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                loop {
                    let locked = engine.lock().await;
                    if !locked.jobs.contains_key(&idle) {
                        e = locked;
                        break;
                    }
                    drop(locked);
                    if tokio::time::Instant::now() > deadline {
                        return Err("Idle backend shutdown has not been confirmed".into());
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
            } else {
                return Err(
                    "Four conversations are running; wait or stop one before starting another"
                        .into(),
                );
            }
        }
        let context = DiscoveryContext::current()?;
        let chat = e.chat(id)?.clone();
        let binary = super::agent_binary(&chat.agent_id)?;
        let executable = chat
            .executable
            .clone()
            .or_else(|| context.executable(binary))
            .ok_or("The selected local agent CLI is not installed")?;
        if !super::is_executable(&executable, cfg!(windows)) {
            return Err("Selected agent program is not an executable file".into());
        }
        let workspace = e.store.directory.join(id).join("workspace");
        private_tree(&workspace)?;
        let profile = e.profile.clone();
        let generation = {
            let running = e.chat(id)?;
            running.generation += 1;
            running.generation
        };
        e.persist(id)?;
        let (commands, receive) = mpsc::channel(32);
        let (events, mut notices) = mpsc::channel(256);
        e.jobs.insert(
            id.into(),
            Job {
                generation,
                full_access: chat.full_access,
                options: chat.options.clone(),
                retiring: false,
                sender: commands.clone(),
            },
        );
        let setup = Setup {
            agent_id: chat.agent_id,
            executable,
            workspace,
            profile,
            browser_session: format!("panel-{id}"),
            backend_session: chat.session_id,
            model: chat.requested_model,
            options: chat.options,
            mcp_executable: std::env::current_exe()
                .map_err(|_| "Cannot locate browser tools executable")?,
            full_access: chat.full_access,
        };
        drop(e);
        let owner = engine.clone();
        let chat_id = id.to_string();
        tokio::spawn(async move {
            backend::run(setup, receive, events).await;
        });
        tokio::spawn(async move {
            while let Some(notice) = notices.recv().await {
                let mut e = owner.lock().await;
                let next = e.notice(&chat_id, generation, notice);
                match next {
                    Ok(Some(message)) => {
                        drop(e);
                        if let Err(error) = start(&owner, &chat_id, message).await {
                            let mut e = owner.lock().await;
                            if let Some(chat) = e.chats.get_mut(&chat_id) {
                                chat.phase = Phase::RecoveryRequired;
                                chat.queue_paused = true;
                                chat.last_error = Some(error);
                            }
                            let _ = e.persist(&chat_id);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if let Some(chat) = e.chats.get_mut(&chat_id) {
                            chat.phase = Phase::RecoveryRequired;
                            chat.queue_paused = true;
                            chat.last_error = Some(error);
                        }
                        let _ = e.persist(&chat_id);
                        if let Some(job) = e.jobs.get(&chat_id) {
                            let _ = job.sender.try_send(BackendCommand::Shutdown);
                        }
                    }
                }
            }
        });
        commands
            .send(BackendCommand::Start(message))
            .await
            .map_err(|_| "Backend process closed before accepting the turn".into())
    })
}

/// Bun-built agents extract native libraries to `$TMPDIR/.bun-<uid>-*` and
/// may reuse them later. Ones extracted while the service still ran under
/// Chrome carry its quarantine and would keep failing Gatekeeper.
#[cfg(target_os = "macos")]
fn release_bun_libraries() {
    use std::os::unix::ffi::OsStrExt;
    let prefix = format!(".bun-{}-", unsafe { libc::getuid() });
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().as_bytes().starts_with(prefix.as_bytes()) {
            continue;
        }
        if let Ok(path) = std::ffi::CString::new(entry.path().as_os_str().as_bytes()) {
            unsafe {
                libc::removexattr(
                    path.as_ptr(),
                    c"com.apple.quarantine".as_ptr(),
                    libc::XATTR_NOFOLLOW,
                )
            };
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 4 || args[0] != "--profile" || args[2] != "--data-dir" {
        return Err("Invalid private agent-service startup arguments".into());
    }
    let profile = args[1].clone();
    let root = PathBuf::from(&args[3]);
    let directory = profile_directory(&root, &profile)?;
    let ownership = private_open(&directory.join("service.lock"), true)?;
    ownership
        .try_lock()
        .map_err(|_| "Another agent service owns this profile")?;
    #[cfg(target_os = "macos")]
    release_bun_libraries();
    let store = Store::open(directory.join("conversations"))?;
    let engine = Arc::new(Mutex::new(Engine {
        store,
        chats: HashMap::new(),
        jobs: HashMap::new(),
        requests: HashMap::new(),
        profile: profile.clone(),
        stop: false,
        schedules: super::schedules::load(&directory.join("conversations"))?,
    }));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|_| "Cannot initialize agent-service runtime")?;
    runtime.block_on(async {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|_|"Cannot bind private agent-service transport")?;
        let endpoint=Endpoint {schema_version:1,profile,address:listener.local_addr().map_err(|_|"Cannot inspect agent-service address")?,capability:format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()),binary_hash:BINARY_HASH.as_ref().map_err(Clone::clone)?.clone(),pid:std::process::id()};
        atomic_write(&directory.join("service.json"),&serde_json::to_vec(&endpoint).map_err(|_|"Cannot encode private service endpoint")?,None)?;
        let scheduler=engine.clone();
        tokio::spawn(async move {
            // Give extension handshakes and profile registration time to settle.
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                if scheduler.lock().await.stop { break; }
                if let Err(error)=schedule_tick(&scheduler).await { eprintln!("Schedule check failed: {error}"); }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        let slots=Arc::new(tokio::sync::Semaphore::new(16));
        loop {
            tokio::select! {
                accepted=listener.accept()=>{
                    let (mut stream,_)=accepted.map_err(|_|"Agent-service accept failed")?;let permit=slots.clone().try_acquire_owned();let capability=endpoint.capability.clone();let engine=engine.clone();
                    tokio::spawn(async move {
                        let Ok(_permit)=permit else{return;};
                        let task=async {
                            let read=async {
                                let mut length=[0u8;4];stream.read_exact(&mut length).await.map_err(|_|())?;let length=u32::from_le_bytes(length) as usize;if length>MAX_FRAME {return Err(());}
                                let mut bytes=vec![0u8;length];stream.read_exact(&mut bytes).await.map_err(|_|())?;serde_json::from_slice::<Request>(&bytes).map_err(|_|())
                            };
                            let request=tokio::time::timeout(Duration::from_secs(10),read).await.map_err(|_|())??;
                            if request.capability!=capability || request.request_id.len()>100 {return Err(());}
                            // A slow operation gets its own budget; a timeout answers with an
                            // error instead of closing the connection without a reply.
                            let result=match tokio::time::timeout(Duration::from_secs(operation_seconds(&request.operation)),dispatch(&engine,&request.operation,&request.params)).await {Ok(result)=>result,Err(_)=>Err(format!("{} did not finish in time; its outcome is unconfirmed",request.operation))};
                            let reply=match result {Ok(result)=>json!({"requestId":request.request_id,"result":result}),Err(error)=>json!({"requestId":request.request_id,"error":error})};let bytes=reply.to_string().into_bytes();if bytes.len()>MAX_FRAME{return Err(());}
                            stream.write_all(&(bytes.len() as u32).to_le_bytes()).await.map_err(|_|())?;stream.write_all(&bytes).await.map_err(|_|())?;Ok::<(),()>(())
                        };let _=task.await;
                    });
                }
                _=tokio::time::sleep(Duration::from_millis(100))=>{let e=engine.lock().await;if e.stop && e.jobs.is_empty() && slots.available_permits()==16 {break;}}
            }
        }
        let _=std::fs::remove_file(directory.join("service.json"));Ok::<(),String>(())
    })
}

#[cfg(test)]
mod approval_tests {
    use super::*;
    #[test]
    fn mcp_tool_approval_is_once_only_and_requires_reviewable_known_server_metadata() {
        let original = json!({"serverName":"chrome_use","mode":"form","_meta":{"codex_approval_kind":"mcp_tool_call","tool_params":{"url":"http://127.0.0.1/fixture"}},"requestedSchema":{"type":"object","properties":{}}});
        let method = "mcpServer/elicitation/request";
        assert!(
            validate_reply(method, &original, &json!({"action":"accept","content":{}})).is_ok()
        );
        assert!(validate_reply(
            method,
            &original,
            &json!({"action":"accept","content":{},"_meta":{"persist":"always"}})
        )
        .is_err());
        for field in ["serverName", "mode", "_meta", "requestedSchema"] {
            let mut unknown = original.clone();
            unknown[field] = Value::Null;
            assert!(
                validate_reply(method, &unknown, &json!({"action":"accept","content":{}})).is_err()
            );
            assert!(validate_reply(
                method,
                &unknown,
                &json!({"action":"decline","content":null})
            )
            .is_ok());
        }
        let mut needed = original;
        needed["requestedSchema"]["properties"] = json!({"answer":{"type":"string"}});
        assert!(validate_reply(method, &needed, &json!({"action":"accept","content":{}})).is_err());
    }
}

/// Close a deleted conversation's own chrome-use browser session (its tabs).
/// A session with no daemon and no tabs returns at once without starting one.
async fn close_browser_session(profile: &str, session: &str) -> Value {
    let Ok(executable) = std::env::current_exe() else {
        return json!({"session":session,"closed":false,"error":"Cannot locate chrome-use"});
    };
    let run = |args: Vec<&str>, seconds: u64| {
        let mut command = tokio::process::Command::new(&executable);
        command
            .args(&args)
            .env("AGENT_BROWSER_BROWSER", profile)
            .env("AGENT_BROWSER_SESSION", session)
            .env("CHROME_USE_NO_UPDATE_CHECK", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        async move {
            matches!(
                tokio::time::timeout(Duration::from_secs(seconds), command.status()).await,
                Ok(Ok(status)) if status.success()
            )
        }
    };
    // A normal close verifies every tab it closes. A record that can no longer
    // be identified (an old target id, or tabs opened before a Chrome restart)
    // keeps that close incomplete forever, so the deleted conversation's
    // session then drops untracked records, and finally its whole record,
    // which stops its daemon. Only the first step claims the tabs are closed.
    if run(vec!["close"], 12).await {
        return json!({"session":session,"closed":true});
    }
    if run(vec!["close", "--forget-untracked"], 10).await {
        return json!({"session":session,"closed":true,"ownershipDropped":true});
    }
    let stopped = run(vec!["session", "stop", session, "--force"], 8).await;
    json!({"session":session,"closed":false,"recordDropped":stopped,"error":if stopped {"chrome-use could not confirm the session's tabs closed; its record was dropped and its daemon stopped"} else {"chrome-use could not close or drop the session"}})
}

#[cfg(test)]
mod acp_approval_tests {
    use super::*;
    #[test]
    fn acp_only_accepts_advertised_options_and_auto_allows_once() {
        let options = json!({"options":[{"optionId":"reject","kind":"reject_once"},{"optionId":"once","kind":"allow_once"},{"optionId":"always","kind":"allow_always"}]});
        let once = automatic_approval("session/request_permission", &options).unwrap();
        assert_eq!(
            once,
            json!({"outcome":{"outcome":"selected","optionId":"once"}})
        );
        assert!(validate_reply("session/request_permission", &options, &once).is_ok());
        for result in [
            json!({"outcome":{"outcome":"selected","optionId":"forged"}}),
            json!({"outcome":{"outcome":"selected","optionId":null}}),
            json!({"outcome":{"outcome":"cancelled","optionId":"once"}}),
        ] {
            assert!(validate_reply("session/request_permission", &options, &result).is_err());
        }
        assert!(validate_reply(
            "session/request_permission",
            &options,
            &json!({"outcome":{"outcome":"cancelled"}})
        )
        .is_ok());
        assert!(automatic_approval(
            "session/request_permission",
            &json!({"options":[{"optionId":"always","kind":"allow_always"}]})
        )
        .is_none());
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleRequest {
    id: Option<String>,
    name: Option<String>,
    prompt: Option<String>,
    agent_id: Option<String>,
    model: Option<String>,
    options: Option<ModelOptions>,
    full_access: Option<bool>,
    rule: Option<super::schedules::Rule>,
    enabled: Option<bool>,
    skill_id: Option<String>,
    caller_conversation: Option<String>,
    acknowledge_unconfirmed: Option<bool>,
}
/// Capability names alone do not establish equal authority across adapters:
/// Codex workspace writes are narrower than Claude/Pi file writes, and Pi
/// has no approval mechanism even when its saved fullAccess flag is false.
fn schedule_permissions_fit(schedule: &super::schedules::Schedule, caller: &Conversation) -> bool {
    use super::options::ToolAccess;
    let requested = schedule
        .options
        .effective_access(&schedule.agent_id, schedule.full_access);
    let allowed = caller
        .options
        .effective_access(&caller.agent_id, caller.full_access);
    let automatic = schedule.full_access || schedule.agent_id == "pi";
    let caller_automatic = caller.full_access || caller.agent_id == "pi";
    requested <= allowed
        && (!automatic || caller_automatic)
        && !(caller.agent_id == "codex"
            && allowed == ToolAccess::WriteFiles
            && schedule.agent_id != "codex"
            && requested >= ToolAccess::WriteFiles)
}

async fn schedule_dispatch(
    engine: &Arc<Mutex<Engine>>,
    operation: &str,
    params: &Value,
) -> Result<Value, String> {
    let p: ScheduleRequest =
        serde_json::from_value(params.clone()).map_err(|_| "Invalid schedule parameters")?;
    let mut e = engine.lock().await;
    let caller = if operation.starts_with("agent") {
        let chat = e
            .chat(
                p.caller_conversation
                    .as_deref()
                    .ok_or("Agent schedule calls require a conversation identity")?,
            )?
            .clone();
        if chat.schedule_id.is_some() && operation != "agentScheduleList" {
            return Err(
                "Scheduled conversations cannot create, modify, delete or run schedules".into(),
            );
        }
        Some(chat)
    } else {
        if p.caller_conversation.is_some() {
            return Err("Panel schedule calls do not accept agent identity".into());
        }
        None
    };
    if operation.ends_with("List") {
        return Ok(
            json!({"schemaVersion":1,"schedules":e.schedules.values().collect::<Vec<_>>(),"timeZone":"hostLocal","serviceRunning":!e.stop}),
        );
    }
    if operation.ends_with("Run") {
        let id = p.id.ok_or("Schedule ID is required")?;
        if let Some(caller) = &caller {
            let schedule = e.schedules.get(&id).ok_or("Schedule not found")?;
            if !schedule_permissions_fit(schedule, caller) {
                return Err(
                    "This schedule has more authority than the calling conversation".into(),
                );
            }
        }
        drop(e);
        return run_schedule(engine, &id).await;
    }
    let id = p.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    uuid::Uuid::parse_str(&id).map_err(|_| "Invalid schedule ID")?;
    if operation.ends_with("Delete") {
        let schedule = e.schedules.get(&id).ok_or("Schedule not found")?;
        if schedule.active_chat.is_some() {
            return Err("Stop and confirm the active run before deleting this schedule".into());
        }
        let mut updated = e.schedules.clone();
        updated.remove(&id);
        super::schedules::persist(&e.store.directory, &updated)?;
        e.schedules = updated;
        return Ok(json!({"schemaVersion":1,"deleted":{"id":id}}));
    }
    if e.schedules.len() >= 64 && !e.schedules.contains_key(&id) {
        return Err("A profile supports at most 64 schedules".into());
    }
    let old = e.schedules.get(&id).cloned();
    let now = super::conversation::now();
    let mut schedule = old.clone().unwrap_or(super::schedules::Schedule {
        id: id.clone(),
        name: String::new(),
        prompt: String::new(),
        agent_id: caller
            .as_ref()
            .map(|c| c.agent_id.clone())
            .unwrap_or_else(|| "codex".into()),
        model: None,
        options: ModelOptions::default(),
        full_access: false,
        rule: super::schedules::Rule::Interval { minutes: 60 },
        enabled: true,
        next_run: 0,
        skill_id: None,
        active_chat: None,
        last_chat: None,
        last_result: None,
        last_run: None,
        finished_at: None,
    });
    if let Some(name) = p.name {
        schedule.name = name;
    }
    if let Some(prompt) = p.prompt {
        schedule.prompt = prompt;
    }
    if let Some(agent) = p.agent_id {
        if agent != schedule.agent_id {
            schedule.model = None;
            schedule.options = ModelOptions::default();
        }
        schedule.agent_id = agent;
    }
    // Explicit updates with a null model use the CLI default.
    if params.get("model").is_some() {
        schedule.model = p.model;
    }
    if let Some(options) = p.options {
        schedule.options = options;
    }
    if let Some(access) = p.full_access {
        schedule.full_access = access;
    }
    if let Some(rule) = p.rule {
        schedule.rule = rule;
    }
    if let Some(enabled) = p.enabled {
        schedule.enabled = enabled;
    }
    if params.get("skillId").is_some() {
        schedule.skill_id = p.skill_id;
    }
    if let Some(caller) = &caller {
        let limit = caller
            .options
            .effective_access(&caller.agent_id, caller.full_access);
        if schedule.options.tool_access.is_none() {
            schedule.options.tool_access = Some(limit);
        }
        if !schedule_permissions_fit(&schedule, caller) {
            return Err(
                "A schedule cannot gain more permissions than the conversation creating it".into(),
            );
        }
        if let Some(original) = &old {
            if !schedule_permissions_fit(original, caller) {
                return Err(
                    "A more powerful schedule can only be changed in the user panel".into(),
                );
            }
        }
    }
    if p.acknowledge_unconfirmed == Some(true) {
        if caller.is_some() {
            return Err("Only the user can acknowledge an unconfirmed previous run".into());
        }
        if let Some(active) = &schedule.active_chat {
            if e.chat(active)?.phase != Phase::RecoveryRequired {
                return Err("The previous run is still confirmed active; stop it first".into());
            }
        }
        schedule.active_chat = None;
        schedule.last_result = Some("previous run acknowledged as unconfirmed".into());
    }
    schedule.validate()?;
    if let Some(skill) = &schedule.skill_id {
        super::skill_library::select(&e.store.directory, &schedule.agent_id, skill)?;
    }
    schedule.next_run = schedule.rule.next(now)?;
    let mut updated = e.schedules.clone();
    updated.insert(id.clone(), schedule.clone());
    super::schedules::persist(&e.store.directory, &updated)?;
    e.schedules = updated;
    Ok(json!({"schemaVersion":1,"schedule":schedule}))
}

async fn run_schedule(engine: &Arc<Mutex<Engine>>, id: &str) -> Result<Value, String> {
    let mut e = engine.lock().await;
    if e.stop {
        return Err("Agent service is stopping; scheduled work was not started".into());
    }
    let schedule = e.schedules.get(id).cloned().ok_or("Schedule not found")?;
    if let Some(active) = &schedule.active_chat {
        let phase = e.chat(active)?.phase.clone();
        if phase.busy() || phase == Phase::RecoveryRequired {
            return Err("The previous scheduled run is active or unconfirmed; it was not overlapped or replayed".into());
        }
    }
    let mut chat = Conversation::new(&schedule.agent_id, schedule.model.clone(), None)?;
    chat.title = schedule.name.clone();
    chat.schedule_id = Some(id.into());
    chat.options = schedule.options.clone();
    chat.full_access = schedule.full_access;
    e.store.create(&chat)?;
    let prepared = (|| -> Result<(Message, Message), String> {
        let workspace = e.store.directory.join(&chat.id).join("workspace");
        private_tree(&workspace)?;
        let skill = schedule
            .skill_id
            .as_deref()
            .map(|skill| {
                super::skill_library::snapshot(
                    &e.store.directory,
                    &schedule.agent_id,
                    skill,
                    &workspace,
                )
            })
            .transpose()?;
        let message = Message {
            id: uuid::Uuid::new_v4().to_string(),
            text: format!("[Scheduled task: {}. Work in your own background browser session; no user tab is attached.]\n\n{}", schedule.name, schedule.prompt),
            page: None, attachments: Vec::new(), skill,
        };
        chat.accept(message.clone())?;
        chat.title = schedule.name.clone();
        let next = chat.begin_next().ok_or("Scheduled turn was not prepared")?;
        e.store.save(&chat)?;
        Ok((message, next))
    })();
    let (message, next) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            e.store.delete(&chat.id).map_err(|cleanup| {
                format!("{error}; unused conversation cleanup failed: {cleanup}")
            })?;
            return Err(error);
        }
    };
    let chat_id = chat.id.clone();
    e.chats.insert(chat_id.clone(), chat);
    e.record(&chat_id,"user",json!({"messageId":message.id,"text":message.text,"scheduleId":id,"skillId":schedule.skill_id}))?;
    let schedule = e.schedules.get_mut(id).ok_or("Schedule disappeared")?;
    schedule.active_chat = Some(chat_id.clone());
    schedule.last_chat = Some(chat_id.clone());
    schedule.last_run = Some(super::conversation::now());
    schedule.last_result = Some("starting".into());
    schedule.finished_at = None;
    super::schedules::persist(&e.store.directory, &e.schedules)?;
    drop(e);
    if let Err(error) = start(engine, &chat_id, next).await {
        let mut e = engine.lock().await;
        let chat = e.chat(&chat_id)?;
        chat.phase = Phase::Failed;
        chat.last_error = Some(error.clone());
        chat.active = None;
        chat.queue_paused = true;
        e.persist(&chat_id)?;
        let schedule = e.schedules.get_mut(id).ok_or("Schedule disappeared")?;
        schedule.last_result = Some(format!("failed: {error}"));
        schedule.active_chat = None;
        schedule.finished_at = Some(super::conversation::now());
        super::schedules::persist(&e.store.directory, &e.schedules)?;
        return Err(format!(
            "Scheduled run was saved but could not start: {error}"
        ));
    }
    Ok(
        json!({"schemaVersion":1,"schedule":engine.lock().await.schedules.get(id),"run":{"conversationId":chat_id,"dispatched":true,"completed":false}}),
    )
}
async fn schedule_tick(engine: &Arc<Mutex<Engine>>) -> Result<(), String> {
    let mut e = engine.lock().await;
    if e.stop {
        return Ok(());
    }
    let now = super::conversation::now();
    let ids: Vec<String> = e.schedules.keys().cloned().collect();
    let mut due = Vec::new();
    let mut changed = false;
    for id in ids {
        if let Some(active) = e.schedules[&id].active_chat.clone() {
            let phase = e.chat(&active)?.phase.clone();
            if phase == Phase::RecoveryRequired {
                if e.schedules[&id].last_result.as_deref() != Some("unconfirmed") {
                    e.schedules.get_mut(&id).unwrap().last_result = Some("unconfirmed".into());
                    changed = true;
                }
            } else if !phase.busy() {
                let error = e.chat(&active)?.last_error.clone();
                let schedule = e.schedules.get_mut(&id).unwrap();
                schedule.active_chat = None;
                schedule.finished_at = Some(now);
                schedule.last_result = Some(match phase {
                    Phase::Idle => "completed".into(),
                    Phase::Interrupted => "interrupted".into(),
                    _ => format!(
                        "failed: {}",
                        error.unwrap_or_else(|| "no confirmed completion".into())
                    ),
                });
                changed = true;
            }
        }
        let schedule = e.schedules.get_mut(&id).unwrap();
        if schedule.enabled && schedule.next_run <= now {
            schedule.next_run = schedule.rule.next(now)?;
            changed = true;
            if schedule.active_chat.is_none() {
                due.push(id);
            } else if schedule.last_result.as_deref() != Some("unconfirmed") {
                schedule.last_result = Some("skipped: previous run still active".into());
            }
        }
    }
    if changed {
        super::schedules::persist(&e.store.directory, &e.schedules)?;
    }
    drop(e);
    for id in due {
        if let Err(error) = run_schedule(engine, &id).await {
            let mut e = engine.lock().await;
            if let Some(schedule) = e.schedules.get_mut(&id) {
                schedule.last_result = Some(format!("failed: {error}"));
                schedule.finished_at = Some(now);
            }
            super::schedules::persist(&e.store.directory, &e.schedules)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    fn engine(root: &Path) -> Arc<Mutex<Engine>> {
        Arc::new(Mutex::new(Engine {
            store: Store::open(root.join("conversations")).unwrap(),
            chats: HashMap::new(),
            jobs: HashMap::new(),
            requests: HashMap::new(),
            profile: "fixture".into(),
            stop: false,
            schedules: std::collections::BTreeMap::new(),
        }))
    }
    #[tokio::test]
    async fn scheduling_does_not_escalate_or_allow_recursive_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(dir.path());
        let created = dispatch(
            &engine,
            "conversationCreate",
            &json!({"agentId":"claude","fullAccess":false,"options":{"toolAccess":"browserOnly"}}),
        )
        .await
        .unwrap();
        let caller = created["conversation"]["id"].as_str().unwrap();
        let save = json!({"callerConversation":caller,"name":"Restricted task","prompt":"Read the page","rule":{"kind":"interval","minutes":10},"options":{"toolAccess":"browserOnly"}});
        let saved = schedule_dispatch(&engine, "agentScheduleSave", &save)
            .await
            .unwrap();
        let id = saved["schedule"]["id"].as_str().unwrap();
        for options in [
            json!({"toolAccess":"readFiles"}),
            json!({"toolAccess":"writeFiles"}),
            json!({"toolAccess":"runCommands"}),
        ] {
            let mut changed = save.clone();
            changed["id"] = id.into();
            changed["options"] = options;
            assert!(schedule_dispatch(&engine, "agentScheduleSave", &changed)
                .await
                .unwrap_err()
                .contains("more permissions"));
        }
        let mut automatic = save.clone();
        automatic["fullAccess"] = true.into();
        assert!(schedule_dispatch(&engine, "agentScheduleSave", &automatic)
            .await
            .is_err());
        engine.lock().await.chat(caller).unwrap().schedule_id = Some(id.into());
        assert!(schedule_dispatch(
            &engine,
            "agentScheduleList",
            &json!({"callerConversation":caller})
        )
        .await
        .is_ok());
        for operation in [
            "agentScheduleSave",
            "agentScheduleDelete",
            "agentScheduleRun",
        ] {
            assert!(schedule_dispatch(
                &engine,
                operation,
                &json!({"callerConversation":caller,"id":id})
            )
            .await
            .unwrap_err()
            .contains("Scheduled conversations"));
        }
    }
    #[tokio::test]
    async fn cross_adapter_permissions_preserve_sandbox_and_approval_limits() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(dir.path());
        let created = dispatch(
            &engine,
            "conversationCreate",
            &json!({"agentId":"codex","fullAccess":true,"options":{"toolAccess":"writeFiles"}}),
        )
        .await
        .unwrap();
        let caller = created["conversation"]["id"].as_str().unwrap();
        let body = json!({"callerConversation":caller,"name":"Cross backend task","prompt":"Read the fixture","agentId":"claude","rule":{"kind":"interval","minutes":10},"options":{"toolAccess":"writeFiles"}});
        assert!(schedule_dispatch(&engine, "agentScheduleSave", &body)
            .await
            .is_err());
        let mut read_only = body.clone();
        read_only["options"]["toolAccess"] = "readFiles".into();
        assert!(schedule_dispatch(&engine, "agentScheduleSave", &read_only)
            .await
            .is_ok());
        {
            let mut e = engine.lock().await;
            let chat = e.chat(caller).unwrap();
            chat.full_access = false;
            chat.options.tool_access = Some(super::super::options::ToolAccess::ReadFiles);
        }
        let mut pi = read_only;
        pi["agentId"] = "pi".into();
        pi["fullAccess"] = false.into();
        assert!(schedule_dispatch(&engine, "agentScheduleSave", &pi)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn unknown_run_outcomes_are_not_overlapped_or_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(dir.path());
        let saved=schedule_dispatch(&engine,"scheduleSave",&json!({"name":"Unknown run","prompt":"Read the page","agentId":"codex","rule":{"kind":"interval","minutes":1},"options":{"toolAccess":"readFiles"}})).await.unwrap();
        let id = saved["schedule"]["id"].as_str().unwrap().to_string();
        let mut chat = Conversation::new("codex", None, None).unwrap();
        chat.phase = Phase::RecoveryRequired;
        chat.schedule_id = Some(id.clone());
        let chat_id = chat.id.clone();
        {
            let mut e = engine.lock().await;
            e.store.create(&chat).unwrap();
            e.chats.insert(chat_id.clone(), chat);
            let schedule = e.schedules.get_mut(&id).unwrap();
            schedule.active_chat = Some(chat_id.clone());
            schedule.next_run = 0;
        }
        assert!(run_schedule(&engine, &id)
            .await
            .unwrap_err()
            .contains("unconfirmed"));
        schedule_tick(&engine).await.unwrap();
        let e = engine.lock().await;
        assert_eq!(e.chats.len(), 1);
        assert_eq!(
            e.schedules[&id].active_chat.as_deref(),
            Some(chat_id.as_str())
        );
        assert_eq!(e.schedules[&id].last_result.as_deref(), Some("unconfirmed"));
        assert!(e.schedules[&id].next_run > super::super::conversation::now());
    }
}
