//! Files a browser tool produced for a turn: screenshots, downloads and
//! recordings. Each lease gets its own output folder in the agent workspace;
//! the host copies the file it finds there, without following links, into a
//! read-only content-addressed blob outside the workspace. A path in a report
//! never authorizes reading it. Delivery to the agent's own requested path is
//! a second, separately recorded effect (`deliver`).
//!
//! This module computes event payloads and state; the engine journals them.
#![allow(dead_code)] // Wired into the service by the integration step.

use super::config_transaction::private_tree;
use super::conversation::Event;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(super) const SCREENSHOT_CAP: u64 = 16 * 1024 * 1024;
pub(super) const DOWNLOAD_CAP: u64 = 100 * 1024 * 1024;
pub(super) const RECORDING_CAP: u64 = 256 * 1024 * 1024;
pub(super) const CONVERSATION_QUOTA: u64 = 1024 * 1024 * 1024;
/// Raw bytes per read; base64 plus envelope stays under the 768 KiB frame.
pub(super) const MAX_CHUNK: u64 = 384 * 1024;
pub(super) const OUTPUT_ROOT: &str = ".chrome-use-out";
const RECORDING_NAME: &str = "recording.webm";
const MAX_NAME: usize = 120;
const MAX_DETAIL: usize = 1000;
const SNIFF_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Kind {
    Screenshot,
    Download,
    Recording,
}
impl Kind {
    pub fn cap(self) -> u64 {
        match self {
            Kind::Screenshot => SCREENSHOT_CAP,
            Kind::Download => DOWNLOAD_CAP,
            Kind::Recording => RECORDING_CAP,
        }
    }
    fn fallback_name(self) -> &'static str {
        match self {
            Kind::Screenshot => "screenshot.png",
            Kind::Download => "download",
            Kind::Recording => RECORDING_NAME,
        }
    }
    fn parse(value: &Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}

fn cap_text(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn canonical_uuid(id: &str) -> Result<String, String> {
    let parsed = uuid::Uuid::parse_str(id).map_err(|_| "Invalid artifact identity")?;
    let canonical = parsed.hyphenated().to_string();
    if canonical != id {
        return Err("Invalid artifact identity".into());
    }
    Ok(canonical)
}

/// One path segment the CLI can write and every OS can name.
pub(super) fn sanitize_name(raw: &str, kind: Kind) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut name: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " ._-()+,=@[]".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    name = name
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .to_string();
    if name.chars().count() > MAX_NAME {
        let extension = extension_of(&name)
            .filter(|e| e.len() <= 10)
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let stem: String = name.chars().take(MAX_NAME - extension.len()).collect();
        name = format!("{}{extension}", stem.trim_end_matches(['.', ' ']));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        name = format!("_{name}");
    }
    if name.is_empty() || name.chars().all(|c| c == '_') {
        return kind.fallback_name().into();
    }
    name
}

fn extension_of(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() || extension.len() > 16 {
        return None;
    }
    Some(extension.to_ascii_lowercase())
}

/// What a capture tool asks for before its lease: the artifact kind, the file
/// name inside the allocated folder, and the agent's own destination, if any.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CaptureRequest {
    pub kind: Kind,
    pub name: String,
    pub requested_destination: Option<String>,
}

pub(super) fn capture_request(tool: &str, arguments: &Value) -> Option<CaptureRequest> {
    let path = arguments["path"]
        .as_str()
        .filter(|p| !p.trim().is_empty())
        .map(String::from);
    let (kind, fallback) = match tool {
        "chrome_use_screenshot" => (Kind::Screenshot, None),
        "chrome_use_download" => (Kind::Download, None),
        "chrome_use_download_url" => (
            Kind::Download,
            arguments["url"].as_str().and_then(|u| {
                let parsed = url::Url::parse(u).ok()?;
                let segment = parsed.path_segments()?.rev().find(|s| !s.is_empty())?;
                urlencoding::decode(segment).ok().map(|s| s.into_owned())
            }),
        ),
        _ => return None,
    };
    let mut name = sanitize_name(path.as_deref().or(fallback.as_deref()).unwrap_or(""), kind);
    // The CLI picks the image encoder from the extension.
    if kind == Kind::Screenshot
        && !matches!(
            extension_of(&name).as_deref(),
            Some("png" | "jpg" | "jpeg" | "webp")
        )
    {
        name = Kind::Screenshot.fallback_name().into();
    }
    Some(CaptureRequest {
        kind,
        name,
        requested_destination: path,
    })
}

/// The tool arguments the CLI runs with: the caller's `path` replaced by the
/// lease's capture path. The original stays in `requested_destination`.
pub(super) fn rewrite_capture_arguments(arguments: &Value, capture_path: &Path) -> Value {
    let mut rewritten = arguments.clone();
    if let Some(object) = rewritten.as_object_mut() {
        object.insert(
            "path".into(),
            Value::String(capture_path.to_string_lossy().into_owned()),
        );
    }
    rewritten
}

/// A lease's allocated output. The folder is created exclusively by the host;
/// its identity binds every later read to that exact folder.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Output {
    pub artifact_id: String,
    pub action_id: Option<String>,
    pub kind: Kind,
    pub name: String,
    pub dir: PathBuf,
    pub dir_identity: (u64, u64),
    pub requested_destination: Option<String>,
}
impl Output {
    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }
    /// One entry of `actionAdmitted.outputs`. `dirIdentity` and
    /// `quotaReserved` let a reload rebuild the lease and the quota.
    pub fn admitted_entry(&self, reserved: u64) -> Value {
        json!({
            "artifactId": self.artifact_id,
            "kind": self.kind,
            "name": self.name,
            "effectiveCapturePath": self.path(),
            "requestedDestination": self.requested_destination,
            "dirIdentity": [self.dir_identity.0, self.dir_identity.1],
            "quotaReserved": reserved,
        })
    }
    pub fn from_admitted(
        agent_cwd: &Path,
        key: &str,
        action_id: Option<&str>,
        entry: &Value,
    ) -> Option<Self> {
        let kind = Kind::parse(&entry["kind"])?;
        let name = entry["name"].as_str()?;
        if sanitize_name(name, kind) != name {
            return None;
        }
        let identity = entry["dirIdentity"].as_array()?;
        Some(Self {
            artifact_id: canonical_uuid(entry["artifactId"].as_str()?).ok()?,
            action_id: action_id.map(String::from),
            kind,
            name: name.into(),
            dir: agent_cwd.join(OUTPUT_ROOT).join(canonical_uuid(key).ok()?),
            dir_identity: (identity.first()?.as_u64()?, identity.get(1)?.as_u64()?),
            requested_destination: entry["requestedDestination"].as_str().map(String::from),
        })
    }
}

/// Create `<agent_cwd>/.chrome-use-out/<key>/` exclusively. `key` is the
/// action id, or the artifact id for a recording.
fn allocate_dir(agent_cwd: &Path, key: &str) -> Result<(PathBuf, (u64, u64)), String> {
    let key = canonical_uuid(key)?;
    let root = agent_cwd.join(OUTPUT_ROOT);
    private_tree(&root)?;
    let dir = root.join(&key);
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .map_err(|_| "Cannot allocate the action output folder")?;
    #[cfg(windows)]
    super::windows::protect_directory(&dir)?;
    let handle = open_dir(&dir).map_err(|_| "Cannot open the action output folder")?;
    Ok((dir, dir_identity(&handle)?))
}

pub(super) fn allocate(
    agent_cwd: &Path,
    action_id: &str,
    request: &CaptureRequest,
) -> Result<Output, String> {
    let (dir, dir_identity) = allocate_dir(agent_cwd, action_id)?;
    Ok(Output {
        artifact_id: uuid::Uuid::new_v4().to_string(),
        action_id: Some(action_id.into()),
        kind: request.kind,
        name: request.name.clone(),
        dir,
        dir_identity,
        requested_destination: request.requested_destination.clone(),
    })
}

pub(super) fn allocate_recording(agent_cwd: &Path) -> Result<Output, String> {
    let artifact_id = uuid::Uuid::new_v4().to_string();
    let (dir, dir_identity) = allocate_dir(agent_cwd, &artifact_id)?;
    Ok(Output {
        artifact_id,
        action_id: None,
        kind: Kind::Recording,
        name: RECORDING_NAME.into(),
        dir,
        dir_identity,
        requested_destination: None,
    })
}

/// Bytes held by ready artifacts plus live reservations, per conversation.
#[derive(Debug, Default)]
pub(super) struct Quota {
    ready: u64,
    reserved: HashMap<String, u64>,
}
impl Quota {
    pub fn used(&self) -> u64 {
        self.ready + self.reserved.values().sum::<u64>()
    }
    /// The kind's cap, or what is left when smaller. Zero is a valid
    /// reservation: the lease is admitted and the copy ends `quotaExceeded`.
    pub fn reserve(&mut self, artifact_id: &str, kind: Kind) -> u64 {
        if let Some(existing) = self.reserved.get(artifact_id) {
            return *existing;
        }
        let amount = kind
            .cap()
            .min(CONVERSATION_QUOTA.saturating_sub(self.used()));
        self.reserved.insert(artifact_id.into(), amount);
        amount
    }
    pub fn reserved_for(&self, artifact_id: &str) -> Option<u64> {
        self.reserved.get(artifact_id).copied()
    }
    pub fn release(&mut self, artifact_id: &str) {
        self.reserved.remove(artifact_id);
    }
    pub fn settle(&mut self, artifact_id: &str, bytes: u64) {
        self.reserved.remove(artifact_id);
        self.ready += bytes;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReadyInfo {
    pub mime: Option<String>,
    pub declared_extension: Option<String>,
    pub type_matches: Option<bool>,
    pub bytes: u64,
    pub digest: String,
}
impl ReadyInfo {
    fn hex(&self) -> &str {
        self.digest.strip_prefix("sha256:").unwrap_or(&self.digest)
    }
    pub fn blob_path(&self, conversation_dir: &Path) -> PathBuf {
        conversation_dir.join("artifacts").join(format!(
            "{}.{}",
            self.hex(),
            blob_extension(self.mime.as_deref())
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Reason {
    Cancelled,
    Interrupted,
    Timeout,
    Failed,
    TooLargeToRetain,
    QuotaExceeded,
    PathMismatch,
    NotRegularFile,
    NotOwned,
    Linked,
    Missing,
    Changed,
    EncoderMissing,
    EncoderFailed,
}
impl Reason {
    /// Native machine codes from downloads (plan 3.1).
    pub fn from_native_code(code: &str) -> Option<Self> {
        match code {
            "download_canceled" => Some(Self::Cancelled),
            "download_interrupted" => Some(Self::Interrupted),
            "download_timeout" => Some(Self::Timeout),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum DetectedBy {
    Report,
    Read,
    TurnEnd,
    Load,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Unavailable {
    pub reason: Reason,
    pub detail: String,
    pub bytes: Option<u64>,
    pub digest: Option<String>,
}
fn unavailable(reason: Reason, detail: &str) -> Unavailable {
    Unavailable {
        reason,
        detail: cap_text(detail, MAX_DETAIL),
        bytes: None,
        digest: None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum State {
    Pending,
    Capturing,
    Finalizing,
    Ready(ReadyInfo),
    Unavailable(Reason),
}
impl State {
    fn terminal(&self) -> bool {
        matches!(self, State::Ready(_) | State::Unavailable(_))
    }
}

#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub message_id: Option<String>,
    pub action_id: Option<String>,
    pub kind: Kind,
    pub name: String,
    pub state: State,
}

/// Per-conversation artifact index and quota, rebuilt from the journal.
#[derive(Debug, Default)]
pub(super) struct Artifacts {
    pub entries: HashMap<String, Entry>,
    pub quota: Quota,
    read_failures: HashSet<String>,
}

impl Artifacts {
    /// Rebuild from the journal and remove leftover `.part` files. Blobs are
    /// not opened here; a missing one is found by the next read.
    pub fn rebuild<'a>(
        conversation_dir: &Path,
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Self {
        let mut this = Self::default();
        for event in events {
            this.apply(&event.kind, &event.data);
        }
        cleanup_incoming(conversation_dir);
        this
    }

    /// Fold one journaled event into the index and quota.
    pub fn apply(&mut self, kind: &str, data: &Value) {
        let message_id = data["messageId"].as_str().map(String::from);
        match kind {
            "actionAdmitted" => {
                for output in data["outputs"].as_array().into_iter().flatten() {
                    let (Some(id), Some(artifact_kind)) =
                        (output["artifactId"].as_str(), Kind::parse(&output["kind"]))
                    else {
                        continue;
                    };
                    self.entries.insert(
                        id.into(),
                        Entry {
                            message_id: message_id.clone(),
                            action_id: data["actionId"].as_str().map(String::from),
                            kind: artifact_kind,
                            name: output["name"].as_str().unwrap_or("").into(),
                            state: State::Pending,
                        },
                    );
                    if let Some(reserved) = output["quotaReserved"].as_u64() {
                        self.quota.reserved.insert(id.into(), reserved);
                    }
                }
            }
            "artifactCreated" => {
                let (Some(id), Some(artifact_kind)) =
                    (data["artifactId"].as_str(), Kind::parse(&data["kind"]))
                else {
                    return;
                };
                let state = if data["state"] == "capturing" {
                    State::Capturing
                } else {
                    State::Pending
                };
                let entry = self.entries.entry(id.into()).or_insert(Entry {
                    message_id: message_id.clone(),
                    action_id: data["actionId"].as_str().map(String::from),
                    kind: artifact_kind,
                    name: data["name"].as_str().unwrap_or("").into(),
                    state: State::Pending,
                });
                if !entry.state.terminal() {
                    entry.state = state;
                }
                if let Some(reserved) = data["quotaReserved"].as_u64() {
                    self.quota.reserved.entry(id.into()).or_insert(reserved);
                }
            }
            "artifactState" => {
                if let Some(entry) = data["artifactId"]
                    .as_str()
                    .and_then(|id| self.entries.get_mut(id))
                {
                    if data["state"] == "finalizing" && !entry.state.terminal() {
                        entry.state = State::Finalizing;
                    }
                }
            }
            "artifactReady" => {
                let Some(id) = data["artifactId"].as_str() else {
                    return;
                };
                let Some(digest) = data["digest"].as_str() else {
                    return;
                };
                let bytes = data["bytes"].as_u64().unwrap_or(0);
                let info = ReadyInfo {
                    mime: data["mime"].as_str().map(String::from),
                    declared_extension: data["declaredExtension"].as_str().map(String::from),
                    type_matches: data["typeMatches"].as_bool(),
                    bytes,
                    digest: digest.into(),
                };
                if let Some(entry) = self.entries.get_mut(id) {
                    if entry.state.terminal() {
                        return;
                    }
                    entry.state = State::Ready(info);
                }
                self.quota.settle(id, bytes);
            }
            "artifactUnavailable" => {
                let Some(id) = data["artifactId"].as_str() else {
                    return;
                };
                let by_read = data["detectedBy"] == "read";
                if by_read {
                    self.read_failures.insert(id.into());
                }
                let reason =
                    serde_json::from_value(data["reason"].clone()).unwrap_or(Reason::Failed);
                if let Some(entry) = self.entries.get_mut(id) {
                    match entry.state {
                        // A lost blob was counted at ready and stays counted,
                        // as the journal recompute does.
                        State::Ready(_) => {
                            if by_read {
                                entry.state = State::Unavailable(reason);
                            }
                            return;
                        }
                        State::Unavailable(_) => return,
                        _ => entry.state = State::Unavailable(reason),
                    }
                }
                self.quota.release(id);
            }
            _ => {}
        }
    }

    /// Register a lease's outputs and reserve quota for each, under the lock.
    pub fn admit(&mut self, message_id: &str, outputs: &[Output]) -> Vec<u64> {
        outputs
            .iter()
            .map(|output| {
                self.entries.insert(
                    output.artifact_id.clone(),
                    Entry {
                        message_id: Some(message_id.into()),
                        action_id: output.action_id.clone(),
                        kind: output.kind,
                        name: output.name.clone(),
                        state: State::Pending,
                    },
                );
                self.quota.reserve(&output.artifact_id, output.kind)
            })
            .collect()
    }

    /// `artifactCreated` for a recording authorized under `message_id`.
    pub fn start_recording(&mut self, message_id: &str, output: &Output) -> Value {
        self.entries.insert(
            output.artifact_id.clone(),
            Entry {
                message_id: Some(message_id.into()),
                action_id: None,
                kind: Kind::Recording,
                name: output.name.clone(),
                state: State::Capturing,
            },
        );
        let reserved = self.quota.reserve(&output.artifact_id, Kind::Recording);
        json!({
            "v": 1,
            "messageId": message_id,
            "artifactId": output.artifact_id,
            "kind": Kind::Recording,
            "name": output.name,
            "state": "capturing",
            "effectiveCapturePath": output.path(),
            "dirIdentity": [output.dir_identity.0, output.dir_identity.1],
            "quotaReserved": reserved,
        })
    }

    pub fn finalizing(&mut self, artifact_id: &str) -> Option<Value> {
        let entry = self.entries.get_mut(artifact_id)?;
        if entry.kind != Kind::Recording || entry.state != State::Capturing {
            return None;
        }
        entry.state = State::Finalizing;
        Some(
            json!({"v": 1, "messageId": entry.message_id, "artifactId": artifact_id, "state": "finalizing"}),
        )
    }

    /// Record an ingest outcome; returns the event to journal, or `None` when
    /// the artifact already reached a terminal state (it never changes back).
    pub fn settle(
        &mut self,
        artifact_id: &str,
        outcome: &Outcome,
    ) -> Option<(&'static str, Value)> {
        match outcome {
            Outcome::Ready(info) => self.ready(artifact_id, info.clone()),
            Outcome::Unavailable(u) => self.fail(artifact_id, u.clone(), DetectedBy::Report),
        }
    }

    fn ready(&mut self, artifact_id: &str, info: ReadyInfo) -> Option<(&'static str, Value)> {
        let entry = self.entries.get_mut(artifact_id)?;
        if entry.state.terminal() {
            return None;
        }
        let event = json!({
            "v": 1,
            "messageId": entry.message_id,
            "artifactId": artifact_id,
            "mime": info.mime,
            "declaredExtension": info.declared_extension,
            "typeMatches": info.type_matches,
            "bytes": info.bytes,
            "digest": info.digest,
        });
        self.quota.settle(artifact_id, info.bytes);
        entry.state = State::Ready(info);
        Some(("artifactReady", event))
    }

    /// Mark a non-terminal artifact unavailable (turn end, crash reload,
    /// native failure code). Terminal artifacts are left as recorded.
    pub fn fail(
        &mut self,
        artifact_id: &str,
        failure: Unavailable,
        detected_by: DetectedBy,
    ) -> Option<(&'static str, Value)> {
        let entry = self.entries.get_mut(artifact_id)?;
        if entry.state.terminal() {
            return None;
        }
        entry.state = State::Unavailable(failure.reason);
        self.quota.release(artifact_id);
        Some((
            "artifactUnavailable",
            unavailable_event(
                entry.message_id.as_deref(),
                artifact_id,
                &failure,
                detected_by,
            ),
        ))
    }

    /// Non-terminal artifacts of one message, for turn-end and reload sweeps.
    pub fn open_for_message(&self, message_id: &str) -> Vec<String> {
        let mut ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| e.message_id.as_deref() == Some(message_id) && !e.state.terminal())
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// A ready artifact of this conversation, for a chunk read off the lock.
    /// Unknown and cross-conversation ids are the same error.
    pub fn readable(&self, artifact_id: &str) -> Result<ReadyInfo, String> {
        match self.entries.get(artifact_id).map(|e| &e.state) {
            Some(State::Ready(info)) => Ok(info.clone()),
            Some(State::Unavailable(_)) => Err("artifact_unavailable".into()),
            Some(_) => Err("artifact_not_ready".into()),
            None => Err("Unknown artifact".into()),
        }
    }

    /// After a failed read: the `artifactUnavailable {detectedBy: "read"}`
    /// event, once per artifact across every reader.
    pub fn read_failed(&mut self, artifact_id: &str, detail: &str) -> Option<Value> {
        let entry = self.entries.get_mut(artifact_id)?;
        if !matches!(entry.state, State::Ready(_)) || !self.read_failures.insert(artifact_id.into())
        {
            return None;
        }
        let State::Ready(info) = &entry.state else {
            return None;
        };
        let failure = Unavailable {
            reason: if detail.contains("changed") {
                Reason::Changed
            } else {
                Reason::Missing
            },
            detail: cap_text(detail, MAX_DETAIL),
            bytes: Some(info.bytes),
            digest: Some(info.digest.clone()),
        };
        entry.state = State::Unavailable(failure.reason);
        Some(unavailable_event(
            entry.message_id.as_deref(),
            artifact_id,
            &failure,
            DetectedBy::Read,
        ))
    }
}

pub(super) fn unavailable_event(
    message_id: Option<&str>,
    artifact_id: &str,
    failure: &Unavailable,
    detected_by: DetectedBy,
) -> Value {
    json!({
        "v": 1,
        "messageId": message_id,
        "artifactId": artifact_id,
        "reason": failure.reason,
        "detail": cap_text(&failure.detail, MAX_DETAIL),
        "bytes": failure.bytes,
        "digest": failure.digest,
        "detectedBy": detected_by,
    })
}

/// Remove `artifacts/incoming/*.part` left by an ingest a crash cut short.
pub(super) fn cleanup_incoming(conversation_dir: &Path) {
    let incoming = conversation_dir.join("artifacts").join("incoming");
    let Ok(entries) = fs::read_dir(&incoming) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_part = path.extension().is_some_and(|e| e == "part");
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if is_part && is_file {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Outcome {
    Ready(ReadyInfo),
    Unavailable(Unavailable),
}

/// Points where a test can change the source mid-ingest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Stage {
    Opened,
    Copying,
}

/// Copy the lease's allocated output into a blob. Runs off the engine lock.
/// `reserved` is the lease's quota reservation for this artifact.
pub(super) fn ingest(
    conversation_dir: &Path,
    output: &Output,
    reported_path: &Path,
    reserved: u64,
) -> Outcome {
    ingest_with(
        conversation_dir,
        output,
        reported_path,
        reserved,
        &mut |_| {},
    )
}

pub(super) fn ingest_with(
    conversation_dir: &Path,
    output: &Output,
    reported_path: &Path,
    reserved: u64,
    hook: &mut dyn FnMut(Stage),
) -> Outcome {
    if reported_path.as_os_str() != output.path().as_os_str() {
        return Outcome::Unavailable(unavailable(
            Reason::PathMismatch,
            "The reported file is not the lease's allocated output",
        ));
    }
    match copy_output(conversation_dir, output, reserved, hook) {
        Ok(outcome) => outcome,
        Err(failure) => Outcome::Unavailable(failure),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Snapshot {
    identity: (u64, u64),
    len: u64,
    modified: (i64, i64),
}

fn copy_output(
    conversation_dir: &Path,
    output: &Output,
    reserved: u64,
    hook: &mut dyn FnMut(Stage),
) -> Result<Outcome, Unavailable> {
    let dir = match open_dir(&output.dir) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(unavailable(Reason::Missing, "The output folder is gone"))
        }
        Err(_) => {
            return Err(unavailable(
                Reason::Changed,
                "The output folder was replaced",
            ))
        }
    };
    if dir_identity(&dir).ok() != Some(output.dir_identity) {
        return Err(unavailable(
            Reason::Changed,
            "The output folder was replaced",
        ));
    }
    let mut source = open_in(&dir, &output.dir, &output.name)?;
    let before = check_source(&source)?;
    hook(Stage::Opened);
    let cap = output.kind.cap();
    let mut hasher = Sha256::new();
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    let retain = before.len <= cap && before.len <= reserved;
    if !retain && before.len <= cap {
        return Err(Unavailable {
            reason: Reason::QuotaExceeded,
            detail: "The conversation's artifact quota is used up".into(),
            bytes: Some(before.len),
            digest: None,
        });
    }
    let incoming = conversation_dir.join("artifacts").join("incoming");
    let part_path = incoming.join(format!("{}.part", output.artifact_id));
    let mut part = if retain {
        private_tree(&incoming).map_err(|e| unavailable(Reason::Failed, &e))?;
        Some(create_part(&part_path).map_err(|e| unavailable(Reason::Failed, &e))?)
    } else {
        None
    };
    let result = (|| {
        let mut limited = Read::by_ref(&mut source).take(before.len + 1);
        loop {
            let n = limited
                .read(&mut buffer)
                .map_err(|_| unavailable(Reason::Failed, "Cannot read the captured file"))?;
            if n == 0 {
                break;
            }
            if head.len() < SNIFF_BYTES {
                let take = (SNIFF_BYTES - head.len()).min(n);
                head.extend_from_slice(&buffer[..take]);
            }
            hasher.update(&buffer[..n]);
            if let Some(part) = part.as_mut() {
                part.write_all(&buffer[..n])
                    .map_err(|_| unavailable(Reason::Failed, "Cannot store the captured file"))?;
            }
            copied += n as u64;
            hook(Stage::Copying);
        }
        let after = snapshot(&source)
            .map_err(|_| unavailable(Reason::Changed, "The captured file changed"))?;
        let still_named = open_in(&dir, &output.dir, &output.name)
            .ok()
            .and_then(|f| snapshot(&f).ok())
            .map(|s| s.identity);
        if after != before || copied != before.len || still_named != Some(before.identity) {
            return Err(unavailable(
                Reason::Changed,
                "The captured file changed while it was copied",
            ));
        }
        let digest = format!("sha256:{}", hex::encode(hasher.finalize()));
        if !retain {
            return Ok(Outcome::Unavailable(Unavailable {
                reason: Reason::TooLargeToRetain,
                detail: format!(
                    "Larger than the {} MiB limit for this kind",
                    cap / 1024 / 1024
                ),
                bytes: Some(copied),
                digest: Some(digest),
            }));
        }
        let mime = sniff(&head).map(String::from);
        let declared_extension = extension_of(&output.name);
        let info = ReadyInfo {
            type_matches: if copied == 0 {
                None
            } else {
                type_matches(mime.as_deref(), declared_extension.as_deref())
            },
            mime,
            declared_extension,
            bytes: copied,
            digest,
        };
        let part = part.take().expect("retained copies have a part file");
        publish(part, &part_path, &info.blob_path(conversation_dir))?;
        Ok(Outcome::Ready(info))
    })();
    if result
        .as_ref()
        .map_or(true, |o| !matches!(o, Outcome::Ready(_)))
    {
        drop(part);
        let _ = fs::remove_file(&part_path);
    }
    result
}

fn check_source(file: &File) -> Result<Snapshot, Unavailable> {
    let meta = file
        .metadata()
        .map_err(|_| unavailable(Reason::Failed, "Cannot inspect the captured file"))?;
    if !meta.file_type().is_file() {
        return Err(unavailable(
            Reason::NotRegularFile,
            "The capture is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(unavailable(
                Reason::NotOwned,
                "The capture belongs to another user",
            ));
        }
        if meta.nlink() != 1 {
            return Err(unavailable(
                Reason::Linked,
                "The capture has another hard link",
            ));
        }
    }
    #[cfg(windows)]
    {
        super::windows::regular(file).map_err(|_| {
            unavailable(Reason::Linked, "The capture is linked or not a plain file")
        })?;
        if !super::windows::owned_by_current_user(file)
            .map_err(|_| unavailable(Reason::NotOwned, "Cannot verify the capture's owner"))?
        {
            return Err(unavailable(
                Reason::NotOwned,
                "The capture belongs to another user",
            ));
        }
    }
    snapshot(file).map_err(|_| unavailable(Reason::Failed, "Cannot inspect the captured file"))
}

fn snapshot(file: &File) -> Result<Snapshot, String> {
    let meta = file.metadata().map_err(|_| "Cannot inspect file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Snapshot {
            identity: (meta.dev(), meta.ino()),
            len: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(Snapshot {
            identity: super::windows::identity(file)?,
            len: meta.len(),
            modified: (meta.last_write_time() as i64, 0),
        })
    }
}

fn dir_identity(dir: &File) -> Result<(u64, u64), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = dir
            .metadata()
            .map_err(|_| "Cannot inspect the output folder")?;
        if !meta.is_dir() {
            return Err("The output folder is not a directory".into());
        }
        Ok((meta.dev(), meta.ino()))
    }
    #[cfg(windows)]
    {
        super::windows::identity(dir)
    }
}

#[cfg(unix)]
fn open_dir(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}
#[cfg(windows)]
fn open_dir(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    super::windows::open_options(&mut options, false);
    let file = options.open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

/// Open `name` inside the already-verified folder without following links.
#[cfg(unix)]
fn open_in(dir: &File, _dir_path: &Path, name: &str) -> Result<File, Unavailable> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = std::ffi::CString::new(name)
        .map_err(|_| unavailable(Reason::Failed, "Invalid output name"))?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::ENOENT) => unavailable(Reason::Missing, "No file was written"),
            Some(libc::ELOOP) | Some(libc::EMLINK) => {
                unavailable(Reason::Linked, "The capture is a symbolic link")
            }
            _ => unavailable(Reason::Failed, "Cannot open the captured file"),
        });
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
#[cfg(windows)]
fn open_in(dir: &File, dir_path: &Path, name: &str) -> Result<File, Unavailable> {
    let mut options = OpenOptions::new();
    options.read(true);
    super::windows::open_options(&mut options, false);
    let file = options.open(dir_path.join(name)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            unavailable(Reason::Missing, "No file was written")
        } else {
            unavailable(Reason::Failed, "Cannot open the captured file")
        }
    })?;
    // No openat on Windows: re-check that the folder is still the one opened.
    let parent = open_dir(dir_path)
        .map_err(|_| unavailable(Reason::Changed, "The output folder was replaced"))?;
    if dir_identity(&parent).ok() != dir_identity(dir).ok() {
        return Err(unavailable(
            Reason::Changed,
            "The output folder was replaced",
        ));
    }
    Ok(file)
}

fn create_part(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    super::windows::open_options(&mut options, true);
    let file = options
        .open(path)
        .map_err(|_| "Cannot create the artifact copy")?;
    #[cfg(windows)]
    super::windows::protect(&file)?;
    Ok(file)
}

/// fsync, make read-only, and name by digest. An existing blob with the same
/// digest is kept and the new copy dropped.
fn publish(part: File, part_path: &Path, blob: &Path) -> Result<(), Unavailable> {
    part.sync_all()
        .map_err(|_| unavailable(Reason::Failed, "Cannot store the captured file"))?;
    if fs::symlink_metadata(blob).is_ok() {
        drop(part);
        let _ = fs::remove_file(part_path);
        return Ok(());
    }
    let mut permissions = part
        .metadata()
        .map_err(|_| unavailable(Reason::Failed, "Cannot store the captured file"))?
        .permissions();
    permissions.set_readonly(true);
    part.set_permissions(permissions)
        .map_err(|_| unavailable(Reason::Failed, "Cannot protect the artifact"))?;
    drop(part);
    if fs::rename(part_path, blob).is_err() {
        if fs::symlink_metadata(blob).is_ok() {
            if let Ok(meta) = fs::metadata(part_path) {
                let mut writable = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                writable.set_readonly(false);
                let _ = fs::set_permissions(part_path, writable);
            }
            let _ = fs::remove_file(part_path);
            return Ok(());
        }
        return Err(unavailable(Reason::Failed, "Cannot publish the artifact"));
    }
    Ok(())
}

/// Magic bytes only; the extension is never trusted.
pub(super) fn sniff(head: &[u8]) -> Option<&'static str> {
    if head.is_empty() {
        return None;
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if head.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Some("application/zip");
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let webm = head.windows(4).any(|w| w == b"webm");
        return Some(if webm {
            "video/webm"
        } else {
            "video/x-matroska"
        });
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return Some("video/mp4");
    }
    let text = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let start = text
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(text.len());
    let lower: Vec<u8> = text[start..]
        .iter()
        .take(64)
        .map(u8::to_ascii_lowercase)
        .collect();
    if [
        &b"<!doctype html"[..],
        b"<html",
        b"<head",
        b"<body",
        b"<script",
        b"<title",
        b"<!--",
    ]
    .iter()
    .any(|tag| lower.starts_with(tag))
    {
        return Some("text/html");
    }
    // A cut multi-byte character at the end of the window is still text.
    let valid = match std::str::from_utf8(text) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && e.valid_up_to() + 4 > text.len(),
    };
    if valid
        && !text
            .iter()
            .any(|b| *b == 0 || (*b < 0x20 && !b"\t\n\r\x0c".contains(b)))
    {
        return Some("text/plain");
    }
    None
}

/// `None` when the extension says nothing checkable.
pub(super) fn type_matches(mime: Option<&str>, extension: Option<&str>) -> Option<bool> {
    let expected: &[&str] = match extension? {
        "png" => &["image/png"],
        "jpg" | "jpeg" => &["image/jpeg"],
        "webp" => &["image/webp"],
        "gif" => &["image/gif"],
        "pdf" => &["application/pdf"],
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "jar" | "apk" => {
            &["application/zip"]
        }
        "webm" => &["video/webm"],
        "mkv" => &["video/x-matroska", "video/webm"],
        "mp4" | "m4v" | "m4a" | "mov" => &["video/mp4"],
        "html" | "htm" => &["text/html"],
        "txt" | "csv" | "tsv" | "json" | "md" | "log" | "xml" | "yaml" | "yml" => &["text/plain"],
        _ => return None,
    };
    Some(mime.is_some_and(|m| expected.contains(&m)))
}

fn blob_extension(mime: Option<&str>) -> &'static str {
    match mime {
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("image/webp") => "webp",
        Some("image/gif") => "gif",
        Some("application/pdf") => "pdf",
        Some("application/zip") => "zip",
        Some("video/webm") => "webm",
        Some("video/x-matroska") => "mkv",
        Some("video/mp4") => "mp4",
        Some("text/html") => "html",
        Some("text/plain") => "txt",
        _ => "bin",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Chunk {
    pub offset: u64,
    pub data: Vec<u8>,
    pub total: u64,
    pub digest: String,
}
impl Chunk {
    pub fn reply(&self, artifact_id: &str) -> Value {
        json!({
            "artifactId": artifact_id,
            "offset": self.offset,
            "data": STANDARD.encode(&self.data),
            "total": self.total,
            "digest": self.digest,
        })
    }
}

/// Read at most `MAX_CHUNK` bytes of a ready blob, off the engine lock.
/// Every call checks the blob is still a plain, single-link file of the
/// indexed length; the call that reaches the end re-hashes the whole blob.
/// `Err` means `artifact_unavailable` (see `Artifacts::read_failed`).
pub(super) fn read_chunk(
    conversation_dir: &Path,
    info: &ReadyInfo,
    offset: u64,
    length: u64,
) -> Result<Chunk, String> {
    if offset > info.bytes {
        return Err("Read offset is past the end of the artifact".into());
    }
    let length = length.min(MAX_CHUNK).min(info.bytes - offset);
    let path = info.blob_path(conversation_dir);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    super::windows::open_options(&mut options, false);
    let mut file = options
        .open(&path)
        .map_err(|_| "artifact_unavailable: the stored file is missing")?;
    let meta = file
        .metadata()
        .map_err(|_| "artifact_unavailable: the stored file is missing")?;
    #[cfg(unix)]
    let single = {
        use std::os::unix::fs::MetadataExt;
        meta.nlink() == 1
    };
    #[cfg(windows)]
    let single = super::windows::regular(&file).is_ok();
    if !meta.is_file() || !single || meta.len() != info.bytes {
        return Err("artifact_unavailable: the stored file changed".into());
    }
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|_| "artifact_unavailable: the stored file changed")?;
    let mut data = Vec::with_capacity(length as usize);
    Read::by_ref(&mut file)
        .take(length)
        .read_to_end(&mut data)
        .map_err(|_| "artifact_unavailable: the stored file changed")?;
    if data.len() as u64 != length {
        return Err("artifact_unavailable: the stored file changed".into());
    }
    if offset + length == info.bytes {
        file.seek(std::io::SeekFrom::Start(0))
            .map_err(|_| "artifact_unavailable: the stored file changed")?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut limited = Read::by_ref(&mut file).take(info.bytes + 1);
        loop {
            let n = limited
                .read(&mut buffer)
                .map_err(|_| "artifact_unavailable: the stored file changed")?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        if format!("sha256:{}", hex::encode(hasher.finalize())) != info.digest {
            return Err("artifact_unavailable: the stored file changed".into());
        }
    }
    Ok(Chunk {
        offset,
        data,
        total: info.bytes,
        digest: info.digest.clone(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum DeliveryState {
    Delivered,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Delivery {
    pub state: DeliveryState,
    pub path: Option<PathBuf>,
    pub digest: Option<String>,
    pub detail: String,
}

/// Copy a ready capture to the agent's requested path (plan 6.2a). Runs in
/// the MCP process with the rights the CLI would have used for that path;
/// destination rules follow `prepare_download_destination`. `delivered` only
/// when the read-back digest equals the artifact digest.
pub(super) fn deliver(capture: &Path, requested: &str, artifact_digest: &str) -> Delivery {
    let failed = |detail: &str, path: Option<PathBuf>| Delivery {
        state: DeliveryState::Failed,
        path,
        digest: None,
        detail: cap_text(detail, MAX_DETAIL),
    };
    let raw = Path::new(requested);
    let raw = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(raw),
            Err(_) => return failed("Cannot resolve the requested location", None),
        }
    };
    let (Some(parent), Some(file_name)) = (raw.parent(), raw.file_name()) else {
        return failed("The requested location has no file name", None);
    };
    if fs::create_dir_all(parent).is_err() {
        return failed("Cannot create the requested folder", None);
    }
    let Ok(parent) = parent.canonicalize() else {
        return failed("Cannot resolve the requested folder", None);
    };
    let destination = parent.join(file_name);
    let temporary = parent.join(format!(".chrome-use-{}.tmp", uuid::Uuid::new_v4()));
    let written = (|| -> Result<String, String> {
        let mut source = File::open(capture).map_err(|_| "Cannot open the captured file")?;
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "Cannot write in the requested folder")?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = source
                .read(&mut buffer)
                .map_err(|_| "Cannot read the captured file")?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            target
                .write_all(&buffer[..n])
                .map_err(|_| "Cannot write the requested file")?;
        }
        target
            .sync_all()
            .map_err(|_| "Cannot write the requested file")?;
        Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
    })();
    let copied = match written {
        Ok(digest) => digest,
        Err(detail) => {
            let _ = fs::remove_file(&temporary);
            return failed(&detail, Some(destination));
        }
    };
    if copied != artifact_digest {
        let _ = fs::remove_file(&temporary);
        return failed(
            "The captured file changed before delivery",
            Some(destination),
        );
    }
    if fs::rename(&temporary, &destination).is_err() {
        let _ = fs::remove_file(&temporary);
        return failed("Cannot replace the requested file", Some(destination));
    }
    let read_back = File::open(&destination).ok().and_then(|mut f| {
        let mut hasher = Sha256::new();
        std::io::copy(&mut f, &mut hasher).ok()?;
        Some(format!("sha256:{}", hex::encode(hasher.finalize())))
    });
    match read_back {
        Some(digest) if digest == artifact_digest => Delivery {
            state: DeliveryState::Delivered,
            path: Some(destination),
            digest: Some(digest),
            detail: String::new(),
        },
        Some(digest) => Delivery {
            digest: Some(digest),
            ..failed(
                "The requested file changed after it was written",
                Some(destination),
            )
        },
        None => failed("Cannot read back the requested file", Some(destination)),
    }
}

/// The service side of `agentDeliveryReport`: `fileDelivered` under the
/// lease's turn, or `None` when the agent asked for no path. `report` is
/// `None` when no delivery was attempted. A reported `delivered` whose digest
/// is not the artifact's is recorded as `failed`.
pub(super) fn file_delivered_event(
    message_id: &str,
    output: &Output,
    report: Option<&Delivery>,
    artifact_digest: Option<&str>,
) -> Option<Value> {
    let requested = output.requested_destination.as_deref()?;
    let (state, digest, detail) = match report {
        None => ("notAttempted", None, "The capture was not delivered"),
        Some(r) if r.state == DeliveryState::Delivered => {
            if r.digest.is_some() && r.digest.as_deref() == artifact_digest {
                ("delivered", r.digest.as_deref(), r.detail.as_str())
            } else {
                (
                    "failed",
                    None,
                    "The delivered file does not match the captured artifact",
                )
            }
        }
        Some(r) => ("failed", None, r.detail.as_str()),
    };
    Some(json!({
        "v": 1,
        "messageId": message_id,
        "actionId": output.action_id,
        "artifactId": output.artifact_id,
        "requestedDestination": requested,
        "state": state,
        "digest": digest,
        "detail": cap_text(detail, MAX_DETAIL),
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        _temp: tempfile::TempDir,
        cwd: PathBuf,
        conversation: PathBuf,
    }
    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let conversation = temp.path().join("conversation");
        let cwd = conversation.join("workspace");
        private_tree(&cwd).unwrap();
        Fixture {
            _temp: temp,
            cwd,
            conversation,
        }
    }
    fn action() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn download(name: &str) -> CaptureRequest {
        capture_request(
            "chrome_use_download",
            &json!({"selector": "#a", "path": format!("/tmp/x/{name}")}),
        )
        .unwrap()
    }
    fn ready(outcome: Outcome) -> ReadyInfo {
        match outcome {
            Outcome::Ready(info) => info,
            other => panic!("expected ready, got {other:?}"),
        }
    }
    fn reason(outcome: &Outcome) -> Reason {
        match outcome {
            Outcome::Unavailable(u) => u.reason,
            other => panic!("expected unavailable, got {other:?}"),
        }
    }
    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n";

    #[test]
    fn names_are_one_safe_segment_and_screenshots_keep_an_image_extension() {
        assert_eq!(sanitize_name("../../etc/passwd", Kind::Download), "passwd");
        assert_eq!(
            sanitize_name("C:\\x\\..\\report.pdf", Kind::Download),
            "report.pdf"
        );
        assert_eq!(sanitize_name("..", Kind::Download), "download");
        assert_eq!(sanitize_name(".hidden", Kind::Download), "hidden");
        assert_eq!(sanitize_name("con.txt", Kind::Download), "_con.txt");
        assert_eq!(sanitize_name("a\u{0}b\nc.pdf", Kind::Download), "a_b_c.pdf");
        let long = format!("{}.pdf", "x".repeat(300));
        let name = sanitize_name(&long, Kind::Download);
        assert!(name.chars().count() <= MAX_NAME && name.ends_with(".pdf"));
        let shot = capture_request("chrome_use_screenshot", &json!({})).unwrap();
        assert_eq!(shot.name, "screenshot.png");
        assert_eq!(shot.requested_destination, None);
        let shot = capture_request("chrome_use_screenshot", &json!({"path": "out/a.jpg"})).unwrap();
        assert_eq!(
            (shot.name.as_str(), shot.requested_destination.as_deref()),
            ("a.jpg", Some("out/a.jpg"))
        );
        let url = capture_request(
            "chrome_use_download_url",
            &json!({"url": "https://example.test/files/Q3%20report.pdf?x=1"}),
        )
        .unwrap();
        assert_eq!(url.name, "Q3 report.pdf");
        assert!(capture_request("chrome_use_click", &json!({})).is_none());
        let rewritten = rewrite_capture_arguments(
            &json!({"path": "mine.pdf", "selector": "#a"}),
            Path::new("/w/o/mine.pdf"),
        );
        assert_eq!(
            rewritten,
            json!({"path": "/w/o/mine.pdf", "selector": "#a"})
        );
    }

    #[test]
    fn same_name_downloads_get_separate_folders_and_their_own_bytes() {
        let f = fixture();
        let mut artifacts = Artifacts::default();
        let a = allocate(&f.cwd, &action(), &download("report.pdf")).unwrap();
        let b = allocate(&f.cwd, &action(), &download("report.pdf")).unwrap();
        assert_ne!(a.path(), b.path());
        assert!(allocate(&f.cwd, a.action_id.as_deref().unwrap(), &download("x")).is_err());
        let reserved = artifacts.admit("m1", &[a.clone(), b.clone()]);
        fs::write(a.path(), [PDF, b"a"].concat()).unwrap();
        fs::write(b.path(), [PDF, b"b"].concat()).unwrap();
        let ra = ready(ingest(&f.conversation, &a, &a.path(), reserved[0]));
        let rb = ready(ingest(&f.conversation, &b, &b.path(), reserved[1]));
        assert_ne!(ra.digest, rb.digest);
        assert_eq!(ra.mime.as_deref(), Some("application/pdf"));
        assert_eq!(ra.type_matches, Some(true));
        artifacts
            .settle(&a.artifact_id, &Outcome::Ready(ra.clone()))
            .unwrap();
        // Sources overwritten afterwards: the blobs keep their own bytes.
        fs::write(a.path(), b"overwritten").unwrap();
        let blob = ra.blob_path(&f.conversation);
        assert_eq!(fs::read(&blob).unwrap(), [PDF, b"a"].concat());
        assert_eq!(fs::metadata(&blob).unwrap().permissions().mode() & 0o222, 0);
        let chunk = read_chunk(
            &f.conversation,
            &artifacts.readable(&a.artifact_id).unwrap(),
            0,
            u64::MAX,
        )
        .unwrap();
        assert_eq!(chunk.data, [PDF, b"a"].concat());
        // Content addressing: the same bytes again keep one blob.
        let c = allocate(&f.cwd, &action(), &download("copy.pdf")).unwrap();
        fs::write(c.path(), [PDF, b"a"].concat()).unwrap();
        let rc = ready(ingest(&f.conversation, &c, &c.path(), DOWNLOAD_CAP));
        assert_eq!(rc.blob_path(&f.conversation), blob);
        assert!(!f
            .conversation
            .join("artifacts/incoming")
            .read_dir()
            .unwrap()
            .any(|_| true));
    }

    #[test]
    fn unsafe_sources_are_refused_without_reading() {
        let f = fixture();
        let out = |name: &str| allocate(&f.cwd, &action(), &download(name)).unwrap();
        let wrong = out("a.pdf");
        fs::write(wrong.path(), PDF).unwrap();
        let elsewhere = f.cwd.join("elsewhere.pdf");
        fs::write(&elsewhere, PDF).unwrap();
        assert_eq!(
            reason(&ingest(&f.conversation, &wrong, &elsewhere, DOWNLOAD_CAP)),
            Reason::PathMismatch
        );
        let link = out("link.pdf");
        std::os::unix::fs::symlink(&elsewhere, link.path()).unwrap();
        assert_eq!(
            reason(&ingest(&f.conversation, &link, &link.path(), DOWNLOAD_CAP)),
            Reason::Linked
        );
        let hard = out("hard.pdf");
        fs::hard_link(&elsewhere, hard.path()).unwrap();
        assert_eq!(
            reason(&ingest(&f.conversation, &hard, &hard.path(), DOWNLOAD_CAP)),
            Reason::Linked
        );
        let fifo = out("fifo.pdf");
        let c = std::ffi::CString::new(fifo.path().to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(
            reason(&ingest(&f.conversation, &fifo, &fifo.path(), DOWNLOAD_CAP)),
            Reason::NotRegularFile
        );
        let missing = out("missing.pdf");
        assert_eq!(
            reason(&ingest(
                &f.conversation,
                &missing,
                &missing.path(),
                DOWNLOAD_CAP
            )),
            Reason::Missing
        );
        // The folder itself swapped for a symlink to another folder.
        let swapped = out("s.pdf");
        let other = f.cwd.join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("s.pdf"), PDF).unwrap();
        fs::remove_dir(&swapped.dir).unwrap();
        std::os::unix::fs::symlink(&other, &swapped.dir).unwrap();
        assert_eq!(
            reason(&ingest(
                &f.conversation,
                &swapped,
                &swapped.path(),
                DOWNLOAD_CAP
            )),
            Reason::Changed
        );
        if unsafe { libc::geteuid() } == 0 {
            let foreign = out("foreign.pdf");
            fs::write(foreign.path(), PDF).unwrap();
            let c = std::ffi::CString::new(foreign.path().to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::chown(c.as_ptr(), 65534, 65534) }, 0);
            assert_eq!(
                reason(&ingest(
                    &f.conversation,
                    &foreign,
                    &foreign.path(),
                    DOWNLOAD_CAP
                )),
                Reason::NotOwned
            );
        }
    }

    #[test]
    fn changes_during_the_copy_are_never_ready() {
        let f = fixture();
        let big: Vec<u8> = [PDF.to_vec(), vec![7u8; 300 * 1024]].concat();
        type Mutation = fn(&Path, &Path);
        let mutations: [(&str, Mutation); 4] = [
            ("overwrite", |p, _| {
                let mut file = OpenOptions::new().write(true).open(p).unwrap();
                file.write_all(b"XXXX").unwrap();
                let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
                file.set_modified(later).unwrap();
            }),
            ("truncate", |p, _| {
                OpenOptions::new()
                    .write(true)
                    .open(p)
                    .unwrap()
                    .set_len(10)
                    .unwrap();
            }),
            ("rename over", |p, scratch| {
                let other = scratch.join("replacement");
                fs::write(&other, b"other bytes").unwrap();
                fs::rename(&other, p).unwrap();
            }),
            ("symlink swap", |p, scratch| {
                let other = scratch.join("target");
                fs::write(&other, b"other bytes").unwrap();
                fs::remove_file(p).unwrap();
                std::os::unix::fs::symlink(&other, p).unwrap();
            }),
        ];
        for (label, mutate) in mutations {
            for stage in [Stage::Opened, Stage::Copying] {
                let output = allocate(&f.cwd, &action(), &download("big.pdf")).unwrap();
                fs::write(output.path(), &big).unwrap();
                let path = output.path();
                let mut done = false;
                let outcome =
                    ingest_with(&f.conversation, &output, &path, DOWNLOAD_CAP, &mut |s| {
                        if s == stage && !done {
                            done = true;
                            mutate(&path, &f.cwd);
                        }
                    });
                assert_eq!(reason(&outcome), Reason::Changed, "{label} at {stage:?}");
            }
        }
        assert!(!f
            .conversation
            .join("artifacts/incoming")
            .read_dir()
            .unwrap()
            .any(|_| true));
    }

    #[test]
    fn caps_quota_sniffing_and_empty_files() {
        let f = fixture();
        let mut quota = Quota::default();
        let first = quota.reserve("a", Kind::Recording);
        assert_eq!(first, RECORDING_CAP);
        for i in 0..10 {
            quota.reserve(&format!("r{i}"), Kind::Recording);
        }
        assert!(quota.used() <= CONVERSATION_QUOTA);
        assert_eq!(quota.reserve("late", Kind::Screenshot), 0);
        // Over the per-file cap: hashed, nothing kept.
        let shot = allocate(
            &f.cwd,
            &action(),
            &capture_request("chrome_use_screenshot", &json!({})).unwrap(),
        )
        .unwrap();
        let file = File::create(shot.path()).unwrap();
        file.set_len(SCREENSHOT_CAP + 1).unwrap();
        drop(file);
        match ingest(&f.conversation, &shot, &shot.path(), SCREENSHOT_CAP) {
            Outcome::Unavailable(u) => {
                assert_eq!(u.reason, Reason::TooLargeToRetain);
                assert_eq!(u.bytes, Some(SCREENSHOT_CAP + 1));
                assert!(u.digest.unwrap().starts_with("sha256:"));
            }
            other => panic!("{other:?}"),
        }
        // Within the cap but over the reservation: not copied.
        let small = allocate(&f.cwd, &action(), &download("a.pdf")).unwrap();
        fs::write(small.path(), PDF).unwrap();
        assert_eq!(
            reason(&ingest(&f.conversation, &small, &small.path(), 4)),
            Reason::QuotaExceeded
        );
        // HTML error page saved as .pdf; empty file.
        let html = allocate(&f.cwd, &action(), &download("bill.pdf")).unwrap();
        fs::write(html.path(), b"\n <!DOCTYPE html><html>error</html>").unwrap();
        let info = ready(ingest(&f.conversation, &html, &html.path(), DOWNLOAD_CAP));
        assert_eq!(
            (info.mime.as_deref(), info.type_matches),
            (Some("text/html"), Some(false))
        );
        let empty = allocate(&f.cwd, &action(), &download("empty.pdf")).unwrap();
        fs::write(empty.path(), b"").unwrap();
        let info = ready(ingest(&f.conversation, &empty, &empty.path(), DOWNLOAD_CAP));
        assert_eq!(
            (info.bytes, info.mime.as_deref(), info.type_matches),
            (0, None, None)
        );
        assert_eq!(
            sniff(b"\x1A\x45\xDF\xA3\x9f\x42\x82\x84webm"),
            Some("video/webm")
        );
        assert_eq!(sniff(b"\0\0\0\x18ftypmp42\0\0\0\0"), Some("video/mp4"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"PK\x03\x04rest"), Some("application/zip"));
        assert_eq!(sniff(b"plain,csv\n1,2\n"), Some("text/plain"));
        assert_eq!(sniff(&[0u8, 1, 2, 3]), None);
        assert_eq!(
            type_matches(Some("application/zip"), Some("docx")),
            Some(true)
        );
        assert_eq!(type_matches(None, Some("pdf")), Some(false));
        assert_eq!(type_matches(Some("text/plain"), Some("weird")), None);
    }

    #[test]
    fn chunks_are_bounded_and_a_changed_or_deleted_blob_is_reported_once() {
        let f = fixture();
        let mut artifacts = Artifacts::default();
        let output = allocate(&f.cwd, &action(), &download("big.bin")).unwrap();
        let reserved = artifacts.admit("m1", std::slice::from_ref(&output))[0];
        let bytes: Vec<u8> = (0..(MAX_CHUNK * 2 + 5)).map(|i| (i % 251) as u8).collect();
        fs::write(output.path(), &bytes).unwrap();
        let outcome = ingest(&f.conversation, &output, &output.path(), reserved);
        let (kind, event) = artifacts.settle(&output.artifact_id, &outcome).unwrap();
        assert_eq!(
            (kind, event["messageId"].as_str()),
            ("artifactReady", Some("m1"))
        );
        assert!(artifacts.settle(&output.artifact_id, &outcome).is_none());
        let info = artifacts.readable(&output.artifact_id).unwrap();
        let mut assembled = Vec::new();
        let mut offset = 0;
        while offset < info.bytes {
            let chunk = read_chunk(&f.conversation, &info, offset, u64::MAX).unwrap();
            assert!(chunk.data.len() as u64 <= MAX_CHUNK);
            assert_eq!(
                (chunk.total, chunk.digest.as_str()),
                (info.bytes, info.digest.as_str())
            );
            offset += chunk.data.len() as u64;
            assembled.extend(chunk.data);
        }
        assert_eq!(assembled, bytes);
        assert!(read_chunk(&f.conversation, &info, info.bytes + 1, 1).is_err());
        // Same length, different bytes: caught by the final re-hash.
        let blob = info.blob_path(&f.conversation);
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o600)).unwrap();
        let mut changed = bytes.clone();
        changed[0] ^= 1;
        fs::write(&blob, &changed).unwrap();
        read_chunk(&f.conversation, &info, 0, MAX_CHUNK).unwrap();
        let error = read_chunk(&f.conversation, &info, MAX_CHUNK * 2, MAX_CHUNK).unwrap_err();
        assert!(error.contains("changed"));
        let event = artifacts.read_failed(&output.artifact_id, &error).unwrap();
        assert_eq!(
            (event["reason"].as_str(), event["detectedBy"].as_str()),
            (Some("changed"), Some("read"))
        );
        assert!(artifacts.read_failed(&output.artifact_id, &error).is_none());
        // A deleted blob, and a second reader after a reload: still once.
        fs::remove_file(&blob).unwrap();
        assert!(read_chunk(&f.conversation, &info, 0, 1).is_err());
        let journal = [
            Event {
                sequence: 1,
                at: 0,
                kind: "actionAdmitted".into(),
                data: json!({"messageId": "m1", "actionId": output.action_id, "outputs": [output.admitted_entry(reserved)]}),
            },
            Event {
                sequence: 2,
                at: 0,
                kind: "artifactReady".into(),
                data: event_ready(&info, &output.artifact_id),
            },
            Event {
                sequence: 3,
                at: 0,
                kind: "artifactUnavailable".into(),
                data: event,
            },
        ];
        let mut reloaded = Artifacts::rebuild(&f.conversation, &journal);
        assert!(reloaded.readable(&output.artifact_id).is_err());
        assert!(reloaded
            .read_failed(&output.artifact_id, "missing")
            .is_none());
        assert_eq!(reloaded.quota.used(), info.bytes);
        assert!(Artifacts::default().readable(&output.artifact_id).is_err());
    }

    fn event_ready(info: &ReadyInfo, id: &str) -> Value {
        json!({"v": 1, "artifactId": id, "mime": info.mime, "declaredExtension": info.declared_extension, "typeMatches": info.type_matches, "bytes": info.bytes, "digest": info.digest})
    }

    #[test]
    fn reload_rebuilds_index_quota_and_removes_part_files() {
        let f = fixture();
        let a = allocate(&f.cwd, &action(), &download("a.pdf")).unwrap();
        let b = allocate(&f.cwd, &action(), &download("b.pdf")).unwrap();
        let incoming = f.conversation.join("artifacts/incoming");
        private_tree(&incoming).unwrap();
        fs::write(incoming.join(format!("{}.part", b.artifact_id)), b"partial").unwrap();
        let ready_info = ReadyInfo {
            mime: Some("application/pdf".into()),
            declared_extension: Some("pdf".into()),
            type_matches: Some(true),
            bytes: 1000,
            digest: format!("sha256:{}", "0".repeat(64)),
        };
        let journal = vec![
            Event {
                sequence: 1,
                at: 0,
                kind: "actionAdmitted".into(),
                data: json!({"messageId": "m1", "actionId": a.action_id, "outputs": [a.admitted_entry(DOWNLOAD_CAP)]}),
            },
            Event {
                sequence: 2,
                at: 0,
                kind: "actionAdmitted".into(),
                data: json!({"messageId": "m1", "actionId": b.action_id, "outputs": [b.admitted_entry(DOWNLOAD_CAP)]}),
            },
            Event {
                sequence: 3,
                at: 0,
                kind: "artifactReady".into(),
                data: event_ready(&ready_info, &a.artifact_id),
            },
        ];
        let mut artifacts = Artifacts::rebuild(&f.conversation, &journal);
        assert!(!incoming.join(format!("{}.part", b.artifact_id)).exists());
        assert_eq!(artifacts.quota.used(), 1000 + DOWNLOAD_CAP);
        assert_eq!(
            artifacts.open_for_message("m1"),
            vec![b.artifact_id.clone()]
        );
        let rebuilt = Output::from_admitted(
            &f.cwd,
            b.action_id.as_deref().unwrap(),
            b.action_id.as_deref(),
            &b.admitted_entry(0),
        )
        .unwrap();
        assert_eq!(rebuilt, b);
        let failure = unavailable(Reason::Interrupted, "report interrupted");
        let (kind, event) = artifacts
            .fail(&b.artifact_id, failure.clone(), DetectedBy::Load)
            .unwrap();
        assert_eq!(
            (kind, event["detectedBy"].as_str()),
            ("artifactUnavailable", Some("load"))
        );
        assert!(artifacts
            .fail(&b.artifact_id, failure.clone(), DetectedBy::TurnEnd)
            .is_none());
        assert!(artifacts
            .fail(&a.artifact_id, failure, DetectedBy::TurnEnd)
            .is_none());
        assert_eq!(artifacts.quota.used(), 1000);
        assert_eq!(
            Reason::from_native_code("download_canceled"),
            Some(Reason::Cancelled)
        );
        assert_eq!(
            Reason::from_native_code("download_interrupted"),
            Some(Reason::Interrupted)
        );
        assert_eq!(
            Reason::from_native_code("download_timeout"),
            Some(Reason::Timeout)
        );
    }

    #[test]
    fn recordings_go_through_capturing_and_finalizing() {
        let f = fixture();
        let mut artifacts = Artifacts::default();
        let output = allocate_recording(&f.cwd).unwrap();
        assert!(output
            .path()
            .ends_with(format!("{}/recording.webm", output.artifact_id)));
        let created = artifacts.start_recording("m1", &output);
        assert_eq!(
            (created["state"].as_str(), created["quotaReserved"].as_u64()),
            (Some("capturing"), Some(RECORDING_CAP))
        );
        assert_eq!(
            artifacts.finalizing(&output.artifact_id).unwrap()["state"],
            "finalizing"
        );
        assert!(artifacts.finalizing(&output.artifact_id).is_none());
        fs::write(
            output.path(),
            b"\x1A\x45\xDF\xA3\x9f\x42\x82\x84webm and more",
        )
        .unwrap();
        let info = ready(ingest(
            &f.conversation,
            &output,
            &output.path(),
            RECORDING_CAP,
        ));
        assert_eq!(
            (info.mime.as_deref(), info.type_matches),
            (Some("video/webm"), Some(true))
        );
    }

    #[test]
    fn delivery_is_verified_by_read_back_and_recorded_separately() {
        let f = fixture();
        let capture = f.cwd.join("capture.pdf");
        fs::write(&capture, PDF).unwrap();
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(PDF)));
        let target = f.cwd.join("deliver/nested/report.pdf");
        let delivered = deliver(&capture, target.to_str().unwrap(), &digest);
        assert_eq!(delivered.state, DeliveryState::Delivered);
        assert_eq!(fs::read(&target).unwrap(), PDF);
        let output = allocate(&f.cwd, &action(), &download("report.pdf")).unwrap();
        let event = file_delivered_event("m1", &output, Some(&delivered), Some(&digest)).unwrap();
        assert_eq!(
            (event["state"].as_str(), event["digest"].as_str()),
            (Some("delivered"), Some(digest.as_str()))
        );
        // The capture changed since the artifact was recorded.
        let stale = deliver(
            &capture,
            target.to_str().unwrap(),
            &format!("sha256:{}", "1".repeat(64)),
        );
        assert_eq!(stale.state, DeliveryState::Failed);
        // An unwritable destination folder.
        let locked = f.cwd.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            let refused = deliver(&capture, locked.join("x.pdf").to_str().unwrap(), &digest);
            assert_eq!(refused.state, DeliveryState::Failed);
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        // The service never records delivered on a digest that is not the artifact's.
        let forged = Delivery {
            digest: Some("sha256:00".into()),
            ..delivered.clone()
        };
        assert_eq!(
            file_delivered_event("m1", &output, Some(&forged), Some(&digest)).unwrap()["state"],
            "failed"
        );
        assert_eq!(
            file_delivered_event("m1", &output, None, Some(&digest)).unwrap()["state"],
            "notAttempted"
        );
        let shot = allocate(
            &f.cwd,
            &action(),
            &capture_request("chrome_use_screenshot", &json!({})).unwrap(),
        )
        .unwrap();
        assert!(file_delivered_event("m1", &shot, Some(&delivered), Some(&digest)).is_none());
    }
}
