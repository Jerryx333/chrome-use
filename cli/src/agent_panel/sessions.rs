//! Agent CLI sessions started outside the panel (a terminal, an IDE) that a
//! panel conversation can continue (`/resume`).
//!
//! The CLIs' own session files are only ever read, never written: a listing
//! reads a bounded head and tail of each file, an import reads a bounded
//! tail. Files are found by session id under each CLI's known session
//! directory; a client never names a path. OpenCode keeps its sessions in a
//! database, so it is asked through its own commands (`acp` session/list
//! and `session export`).

use super::DiscoveryContext;
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A session written to within this window counts as running elsewhere.
pub(super) const RUNNING_WINDOW: Duration = Duration::from_secs(60);
/// Bytes read from the start of a session file for its identity and title.
const HEAD_BYTES: u64 = 256 * 1024;
/// Bytes read from the end of a session file for its title and history.
const TITLE_TAIL_BYTES: u64 = 64 * 1024;
const HISTORY_TAIL_BYTES: u64 = 2 * 1024 * 1024;
/// Newest session files considered by one listing.
const MAX_FILES: usize = 400;
/// Imported messages, newest last; earlier ones are counted, not kept.
pub(super) const MAX_IMPORTED: usize = 40;
/// Characters kept of one imported message.
const MAX_MESSAGE_CHARS: usize = 4000;
/// Characters kept of all imported messages together.
const MAX_IMPORTED_CHARS: usize = 64 * 1024;
/// Bytes of `opencode session export` output accepted.
const MAX_EXPORT: usize = 32 * 1024 * 1024;

/// One CLI session on this machine. `title` is raw session text; callers
/// redact it before it leaves the service.
#[derive(Clone, Debug)]
pub(super) struct Session {
    pub agent: String,
    pub id: String,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    /// Milliseconds since the epoch.
    pub last_active: u64,
    pub running: bool,
    /// The session file (Claude, Codex, Pi); OpenCode has none.
    pub file: Option<PathBuf>,
    /// The model of the session's latest turn, where the CLI records one
    /// (Codex): a resumed thread restores that model's default effort.
    pub model: Option<String>,
}

/// One message of an imported transcript.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Turn {
    pub role: &'static str,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct History {
    pub turns: Vec<Turn>,
    /// Earlier messages that were found but not imported.
    pub omitted: usize,
    /// Records that could not be read.
    pub unparsed: usize,
    /// Only the end of a large file was read: anything before it is not
    /// recorded.
    pub partial: bool,
}

/// Where each CLI keeps its sessions, honoring the CLIs' own overrides.
#[derive(Clone, Debug)]
pub(super) struct Roots {
    /// `~/.claude` (`CLAUDE_CONFIG_DIR`): `projects/<dir>/<id>.jsonl`, and
    /// `sessions/<pid>.json` for the sessions of live processes.
    pub claude: PathBuf,
    /// `~/.codex` (`CODEX_HOME`): `sessions/YYYY/MM/DD/rollout-*-<id>.jsonl`.
    pub codex: PathBuf,
    /// `~/.pi/agent/sessions` (`PI_CODING_AGENT_SESSION_DIR`,
    /// `PI_CODING_AGENT_DIR`): `<dir>/<time>_<id>.jsonl`.
    pub pi: PathBuf,
}

impl Roots {
    pub(super) fn current() -> Result<Self, String> {
        Ok(Self::from_context(&DiscoveryContext::current()?))
    }
    fn from_context(context: &DiscoveryContext) -> Self {
        let pi = context
            .variable_path("PI_CODING_AGENT_SESSION_DIR")
            .or_else(|| {
                context
                    .variable_path("PI_CODING_AGENT_DIR")
                    .map(|p| p.join("sessions"))
            })
            .unwrap_or_else(|| context.home.join(".pi/agent/sessions"));
        Self {
            claude: context
                .variable_path("CLAUDE_CONFIG_DIR")
                .unwrap_or_else(|| context.home.join(".claude")),
            codex: context
                .variable_path("CODEX_HOME")
                .unwrap_or_else(|| context.home.join(".codex")),
            pi,
        }
    }
}

/// Whether the agent's sessions can be continued from the panel.
pub(super) fn supported(agent: &str) -> bool {
    matches!(agent, "claude" | "codex" | "pi" | "opencode")
}

/// A session id in the agent's own format. File-based ids are canonical
/// UUIDs, so an id can never name another path.
pub(super) fn valid_id(agent: &str, id: &str) -> bool {
    match agent {
        "opencode" => {
            id.len() <= 64
                && id.strip_prefix("ses_").is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|c| c.is_ascii_alphanumeric())
                })
        }
        _ => uuid::Uuid::parse_str(id).is_ok_and(|u| u.hyphenated().to_string() == id),
    }
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn recent(last_active: u64) -> bool {
    millis(SystemTime::now()).saturating_sub(last_active) < RUNNING_WINDOW.as_millis() as u64
}

/// Regular files directly in `directory` (no symlinks), with their mtimes.
fn files_in(directory: &Path, keep: impl Fn(&str) -> bool) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !keep(&name) {
                return None;
            }
            let meta = std::fs::symlink_metadata(entry.path()).ok()?;
            meta.is_file()
                .then(|| (entry.path(), meta.modified().unwrap_or(UNIX_EPOCH)))
        })
        .collect()
}

/// Directories directly in `directory` (no symlinks), by name.
fn directories_in(directory: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let meta = std::fs::symlink_metadata(entry.path()).ok()?;
            let name = entry.file_name().into_string().ok()?;
            meta.is_dir().then(|| (name, entry.path()))
        })
        .collect()
}

/// The session id a file name carries, if the agent named the file.
fn file_id(agent: &str, name: &str) -> Option<String> {
    let stem = name.strip_suffix(".jsonl")?;
    let id = match agent {
        "claude" => stem,
        // rollout-2026-10-15T23-53-38-<id>
        "codex" => {
            let rest = stem.strip_prefix("rollout-")?;
            rest.get(rest.len().checked_sub(36)?..)?
        }
        // 2026-10-15T14-50-19-265Z_<id>
        "pi" => stem.rsplit_once('_')?.1,
        _ => return None,
    };
    valid_id(agent, id).then(|| id.to_string())
}

/// Session files of a file-based agent, newest first.
fn session_files(agent: &str, roots: &Roots) -> Vec<(String, PathBuf, SystemTime)> {
    let mut files: Vec<(String, PathBuf, SystemTime)> = Vec::new();
    fn add(files: &mut Vec<(String, PathBuf, SystemTime)>, agent: &str, directory: &Path) {
        for (path, modified) in files_in(directory, |n| file_id(agent, n).is_some()) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(id) = file_id(agent, name) {
                files.push((id, path, modified));
            }
        }
    }
    match agent {
        "claude" => {
            for (_, directory) in directories_in(&roots.claude.join("projects")) {
                add(&mut files, agent, &directory);
            }
        }
        "pi" => {
            for (_, directory) in directories_in(&roots.pi) {
                add(&mut files, agent, &directory);
            }
        }
        "codex" => {
            // Newest days first; stop once enough files are found.
            let mut years = directories_in(&roots.codex.join("sessions"));
            years.sort_by(|a, b| b.0.cmp(&a.0));
            'days: for (_, year) in years {
                let mut months = directories_in(&year);
                months.sort_by(|a, b| b.0.cmp(&a.0));
                for (_, month) in months {
                    let mut days = directories_in(&month);
                    days.sort_by(|a, b| b.0.cmp(&a.0));
                    for (_, day) in days {
                        add(&mut files, agent, &day);
                        if files.len() >= MAX_FILES {
                            break 'days;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.2));
    files
}

/// Read at most `limit` bytes from `offset`.
fn read_range(path: &Path, offset: u64, limit: u64) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Complete JSON lines of the first `HEAD_BYTES` of a file.
fn head(path: &Path) -> Vec<Value> {
    let bytes = read_range(path, 0, HEAD_BYTES).unwrap_or_default();
    let complete = match bytes.iter().rposition(|b| *b == b'\n') {
        Some(end) => &bytes[..end],
        None if (bytes.len() as u64) < HEAD_BYTES => &bytes[..],
        None => &[],
    };
    complete
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect()
}

/// Lines of the last `limit` bytes of a file, whether the read started
/// mid-file, and how many lines did not parse.
fn tail(path: &Path, limit: u64) -> (Vec<Value>, bool, usize) {
    let length = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let offset = length.saturating_sub(limit);
    let bytes = read_range(path, offset, limit).unwrap_or_default();
    // A read that starts mid-file starts mid-line; that line is dropped.
    let start = if offset > 0 {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |p| p + 1)
    } else {
        0
    };
    let mut unparsed = 0;
    let values = bytes[start..]
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .filter_map(|line| {
            let value = serde_json::from_slice(line).ok();
            if value.is_none() {
                unparsed += 1;
            }
            value
        })
        .collect();
    (values, offset > 0, unparsed)
}

/// Text of a message content: a string, or the text blocks of an array.
fn content_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| {
                matches!(
                    b["type"].as_str(),
                    Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Text the CLI injected into a user message (command wrappers, system
/// reminders, environment context, project instructions), not typed by the
/// user.
fn injected(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with('<') || text.starts_with("# AGENTS.md instructions")
}

fn user_text(content: &Value) -> Option<String> {
    content_text(content).filter(|t| !injected(t))
}

fn claude_record_user(record: &Value) -> Option<String> {
    if record["type"] != "user" || record["isMeta"] == true || record["isSidechain"] == true {
        return None;
    }
    user_text(&record["message"]["content"])
}

fn read_claude(id: &str, path: &Path, modified: SystemTime, live: &[(String, i64)]) -> Session {
    let lines = head(path);
    let cwd = lines
        .iter()
        .find_map(|r| r["cwd"].as_str())
        .map(PathBuf::from);
    let (tail_lines, _, _) = tail(path, TITLE_TAIL_BYTES);
    let named = tail_lines
        .iter()
        .chain(lines.iter())
        .rev()
        .find_map(|r| {
            (r["type"] == "custom-title")
                .then(|| r["customTitle"].as_str())
                .flatten()
        })
        .map(String::from);
    let title = named.or_else(|| lines.iter().find_map(claude_record_user));
    let last_active = millis(modified);
    let running = recent(last_active)
        || live
            .iter()
            .any(|(session, pid)| session == id && pid_alive(*pid));
    Session {
        agent: "claude".into(),
        id: id.into(),
        title,
        cwd,
        last_active,
        running,
        file: Some(path.into()),
        model: None,
    }
}

/// `(sessionId, pid)` of Claude processes that registered themselves.
fn claude_live(roots: &Roots) -> Vec<(String, i64)> {
    files_in(&roots.claude.join("sessions"), |n| n.ends_with(".json"))
        .into_iter()
        .filter_map(|(path, _)| {
            let bytes = read_range(&path, 0, 64 * 1024)?;
            let value: Value = serde_json::from_slice(&bytes).ok()?;
            Some((
                value["sessionId"].as_str()?.to_string(),
                value["pid"].as_i64()?,
            ))
        })
        .collect()
}

fn pid_alive(pid: i64) -> bool {
    u32::try_from(pid).is_ok_and(|pid| pid > 0 && crate::connection::is_pid_alive(pid))
}

/// Thread names Codex keeps in `session_index.jsonl`; later lines win.
fn codex_names(roots: &Roots) -> std::collections::HashMap<String, String> {
    let (lines, _, _) = tail(&roots.codex.join("session_index.jsonl"), 1024 * 1024);
    lines
        .into_iter()
        .filter_map(|l| {
            Some((
                l["id"].as_str()?.to_string(),
                l["thread_name"].as_str()?.to_string(),
            ))
        })
        .collect()
}

fn codex_record_user(record: &Value) -> Option<String> {
    let payload = &record["payload"];
    if record["type"] != "response_item"
        || payload["type"] != "message"
        || payload["role"] != "user"
    {
        return None;
    }
    user_text(&payload["content"])
}

/// A Codex session, or none when the panel started it (those are panel
/// conversations already).
fn read_codex(
    id: &str,
    path: &Path,
    modified: SystemTime,
    names: &std::collections::HashMap<String, String>,
) -> Option<Session> {
    let lines = head(path);
    let meta = lines
        .iter()
        .find(|r| r["type"] == "session_meta")
        .map(|r| &r["payload"]);
    if meta.is_some_and(|m| m["originator"] == "chrome_use_panel") {
        return None;
    }
    if meta.is_some_and(|m| m["id"].as_str().is_some_and(|m| m != id)) {
        return None;
    }
    let cwd = meta.and_then(|m| m["cwd"].as_str()).map(PathBuf::from);
    let title = names
        .get(id)
        .cloned()
        .or_else(|| lines.iter().find_map(codex_record_user));
    let (tail_lines, _, _) = tail(path, TITLE_TAIL_BYTES);
    let model = tail_lines
        .iter()
        .chain(lines.iter())
        .rev()
        .find_map(|r| {
            (r["type"] == "turn_context")
                .then(|| r["payload"]["model"].as_str())
                .flatten()
        })
        .filter(|m| !m.is_empty() && m.len() <= 256)
        .map(String::from);
    let last_active = millis(modified);
    Some(Session {
        agent: "codex".into(),
        id: id.into(),
        title,
        cwd,
        last_active,
        running: recent(last_active),
        file: Some(path.into()),
        model,
    })
}

fn pi_record_user(record: &Value) -> Option<String> {
    let message = &record["message"];
    (record["type"] == "message" && message["role"] == "user")
        .then(|| user_text(&message["content"]))
        .flatten()
}

fn read_pi(id: &str, path: &Path, modified: SystemTime) -> Option<Session> {
    let lines = head(path);
    let header = lines.first().filter(|r| r["type"] == "session")?;
    if header["id"].as_str() != Some(id) {
        return None;
    }
    let (tail_lines, _, _) = tail(path, TITLE_TAIL_BYTES);
    let named = tail_lines
        .iter()
        .chain(lines.iter())
        .rev()
        .find_map(|r| {
            (r["type"] == "session_info")
                .then(|| r["name"].as_str())
                .flatten()
        })
        .map(String::from);
    let last_active = millis(modified);
    Some(Session {
        agent: "pi".into(),
        id: id.into(),
        title: named.or_else(|| lines.iter().find_map(pi_record_user)),
        cwd: header["cwd"].as_str().map(PathBuf::from),
        last_active,
        running: recent(last_active),
        file: Some(path.into()),
        model: None,
    })
}

fn read_file_session(
    agent: &str,
    id: &str,
    path: &Path,
    modified: SystemTime,
    context: &FileContext,
) -> Option<Session> {
    match agent {
        "claude" => Some(read_claude(id, path, modified, &context.claude_live)),
        "codex" => read_codex(id, path, modified, &context.codex_names),
        "pi" => read_pi(id, path, modified),
        _ => None,
    }
}

/// What a listing reads once rather than per file.
#[derive(Default)]
struct FileContext {
    claude_live: Vec<(String, i64)>,
    codex_names: std::collections::HashMap<String, String>,
}

impl FileContext {
    fn new(agent: &str, roots: &Roots) -> Self {
        match agent {
            "claude" => Self {
                claude_live: claude_live(roots),
                ..Self::default()
            },
            "codex" => Self {
                codex_names: codex_names(roots),
                ..Self::default()
            },
            _ => Self::default(),
        }
    }
}

/// Whether a session matches a picker search: its id, title or directory
/// contains the query, ignoring case.
pub(super) fn matches(session: &Session, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    session.id.to_lowercase().contains(&query)
        || session
            .title
            .as_deref()
            .is_some_and(|t| t.to_lowercase().contains(&query))
        || session
            .cwd
            .as_ref()
            .is_some_and(|c| c.to_string_lossy().to_lowercase().contains(&query))
}

/// Recent sessions of a file-based agent, newest first. Sessions whose
/// directory is inside `panel_root` belong to panel conversations and are
/// left out.
pub(super) fn list_files(
    agent: &str,
    roots: &Roots,
    query: &str,
    limit: usize,
    panel_root: &Path,
) -> Vec<Session> {
    let context = FileContext::new(agent, roots);
    let mut found = Vec::new();
    for (id, path, modified) in session_files(agent, roots).into_iter().take(MAX_FILES) {
        let Some(session) = read_file_session(agent, &id, &path, modified, &context) else {
            continue;
        };
        if session
            .cwd
            .as_ref()
            .is_some_and(|c| c.starts_with(panel_root))
            || !matches(&session, query)
        {
            continue;
        }
        found.push(session);
        if found.len() >= limit {
            break;
        }
    }
    found
}

/// Working directories of an agent's newest sessions with when each was
/// last written, for the panel's list of recent project folders. Only the
/// start of each file is read; the panel's own sessions are left out.
pub(super) fn recent_directories(agent: &str, roots: &Roots, limit: usize) -> Vec<(PathBuf, u64)> {
    session_files(agent, roots)
        .into_iter()
        .take(limit)
        .filter_map(|(_, path, modified)| {
            let lines = head(&path);
            let cwd = match agent {
                "codex" => {
                    let meta = &lines.iter().find(|r| r["type"] == "session_meta")?["payload"];
                    if meta["originator"] == "chrome_use_panel" {
                        return None;
                    }
                    meta["cwd"].as_str()
                }
                _ => lines.iter().find_map(|r| r["cwd"].as_str()),
            }?;
            Some((PathBuf::from(cwd), millis(modified)))
        })
        .collect()
}

/// One file-based session by id, its file confirmed to be inside the agent's
/// session directory.
pub(super) fn find_file(agent: &str, roots: &Roots, id: &str) -> Result<Session, String> {
    if !valid_id(agent, id) {
        return Err("Invalid session id for this agent".into());
    }
    let root = match agent {
        "claude" => roots.claude.join("projects"),
        "codex" => roots.codex.join("sessions"),
        "pi" => roots.pi.clone(),
        _ => return Err("This agent's sessions cannot be continued in the panel".into()),
    };
    let (_, path, modified) = session_files(agent, roots)
        .into_iter()
        .find(|(found, _, _)| found == id)
        .ok_or("That session was not found on this computer")?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "The session directory is unavailable")?;
    let canonical = path
        .canonicalize()
        .map_err(|_| "The session file is unavailable")?;
    if !canonical.starts_with(&canonical_root) {
        return Err("The session file is outside the agent's session directory".into());
    }
    read_file_session(
        agent,
        id,
        &canonical,
        modified,
        &FileContext::new(agent, roots),
    )
    .ok_or_else(|| "That session cannot be continued in the panel".into())
}

/// Keep the newest `MAX_IMPORTED` messages, bounded in size.
fn bound(mut turns: Vec<Turn>, mut history: History) -> History {
    // Consecutive tool summaries read as one line.
    let mut merged: Vec<Turn> = Vec::new();
    for turn in turns.drain(..) {
        match merged.last_mut() {
            Some(last) if last.role == turn.role && turn.role != "user" => {
                last.text
                    .push_str(if turn.role == "tool" { ", " } else { "\n\n" });
                last.text.push_str(&turn.text);
            }
            _ => merged.push(turn),
        }
    }
    let keep = merged.len().min(MAX_IMPORTED);
    history.omitted += merged.len() - keep;
    let mut kept: Vec<Turn> = merged.split_off(merged.len() - keep);
    let mut total = 0usize;
    for turn in kept.iter_mut().rev() {
        let count = turn.text.chars().count();
        if count > MAX_MESSAGE_CHARS {
            turn.text = turn
                .text
                .chars()
                .take(MAX_MESSAGE_CHARS)
                .collect::<String>()
                + "\n[shortened]";
        }
        total += turn.text.chars().count();
    }
    while total > MAX_IMPORTED_CHARS && !kept.is_empty() {
        total -= kept[0].text.chars().count();
        kept.remove(0);
        history.omitted += 1;
    }
    history.turns = kept;
    history
}

fn tool_turn(name: Option<&str>) -> Option<Turn> {
    let name = name.filter(|n| !n.is_empty())?;
    Some(Turn {
        role: "tool",
        text: name.chars().take(120).collect(),
    })
}

fn claude_turns(records: &[Value]) -> Vec<Turn> {
    let mut turns = Vec::new();
    for record in records {
        if record["isSidechain"] == true {
            continue;
        }
        if let Some(text) = claude_record_user(record) {
            turns.push(Turn { role: "user", text });
        } else if record["type"] == "assistant" {
            for block in record["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
            {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"]
                            .as_str()
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                        {
                            turns.push(Turn {
                                role: "assistant",
                                text: text.into(),
                            });
                        }
                    }
                    Some("tool_use") => turns.extend(tool_turn(block["name"].as_str())),
                    _ => {}
                }
            }
        }
    }
    turns
}

fn codex_turns(records: &[Value]) -> Vec<Turn> {
    let mut turns = Vec::new();
    for record in records {
        let payload = &record["payload"];
        if record["type"] != "response_item" {
            continue;
        }
        match (payload["type"].as_str(), payload["role"].as_str()) {
            (Some("message"), Some("user")) => {
                if let Some(text) = user_text(&payload["content"]) {
                    turns.push(Turn { role: "user", text });
                }
            }
            (Some("message"), Some("assistant")) => {
                if let Some(text) = content_text(&payload["content"]) {
                    turns.push(Turn {
                        role: "assistant",
                        text,
                    });
                }
            }
            (Some("function_call" | "custom_tool_call"), _) => {
                turns.extend(tool_turn(payload["name"].as_str()))
            }
            (Some("local_shell_call"), _) => turns.extend(tool_turn(Some("shell"))),
            _ => {}
        }
    }
    turns
}

fn pi_turns(records: &[Value]) -> Vec<Turn> {
    let mut turns = Vec::new();
    for record in records {
        if let Some(text) = pi_record_user(record) {
            turns.push(Turn { role: "user", text });
            continue;
        }
        let message = &record["message"];
        if record["type"] != "message" || message["role"] != "assistant" {
            continue;
        }
        for block in message["content"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"]
                        .as_str()
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                    {
                        turns.push(Turn {
                            role: "assistant",
                            text: text.into(),
                        });
                    }
                }
                Some("toolCall") => turns.extend(tool_turn(block["name"].as_str())),
                _ => {}
            }
        }
    }
    turns
}

/// The end of a file-based session's transcript.
pub(super) fn file_history(session: &Session) -> History {
    let Some(path) = &session.file else {
        return History::default();
    };
    let (records, partial, unparsed) = tail(path, HISTORY_TAIL_BYTES);
    let turns = match session.agent.as_str() {
        "claude" => claude_turns(&records),
        "codex" => codex_turns(&records),
        "pi" => pi_turns(&records),
        _ => Vec::new(),
    };
    bound(
        turns,
        History {
            partial,
            unparsed,
            ..History::default()
        },
    )
}

/// The transcript in an `opencode session export` document.
pub(super) fn opencode_history(export: &Value) -> History {
    let mut turns = Vec::new();
    let mut unparsed = 0;
    for message in export["messages"].as_array().into_iter().flatten() {
        match message["type"].as_str() {
            Some("user") => {
                // Turns the panel sent carry its instructions first (ACP has
                // no separate field for them); only the user's text is shown.
                let text = message["text"].as_str().map(|t| {
                    t.strip_prefix(super::backend::PROMPT)
                        .and_then(|rest| rest.strip_prefix("\n\n"))
                        .unwrap_or(t)
                });
                if let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) {
                    turns.push(Turn {
                        role: "user",
                        text: text.into(),
                    });
                }
            }
            Some("assistant") => {
                for block in message["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = block["text"]
                                .as_str()
                                .map(str::trim)
                                .filter(|t| !t.is_empty())
                            {
                                turns.push(Turn {
                                    role: "assistant",
                                    text: text.into(),
                                });
                            }
                        }
                        Some("tool") => turns.extend(tool_turn(block["tool"].as_str())),
                        _ => {}
                    }
                }
            }
            Some(_) => {}
            None => unparsed += 1,
        }
    }
    if export["messages"].as_array().is_none() {
        unparsed += 1;
    }
    bound(
        turns,
        History {
            unparsed,
            ..History::default()
        },
    )
}

/// A session from an OpenCode ACP `session/list` entry.
pub(super) fn opencode_session(entry: &Value) -> Option<Session> {
    let id = entry["sessionId"]
        .as_str()
        .filter(|id| valid_id("opencode", id))?;
    let last_active = entry["updatedAt"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp_millis().max(0) as u64)
        .unwrap_or(0);
    Some(Session {
        agent: "opencode".into(),
        id: id.into(),
        title: entry["title"]
            .as_str()
            .filter(|t| !t.trim().is_empty())
            .map(String::from),
        cwd: entry["cwd"].as_str().map(PathBuf::from),
        last_active,
        running: recent(last_active),
        file: None,
        model: None,
    })
}

/// A command that runs the OpenCode CLI the way the panel's backends do.
fn opencode_command(executable: &Path, directory: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(executable);
    command
        .current_dir(directory)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = super::agent_search_path() {
        command.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    command
}

type Lines = tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>;

/// One JSON-RPC request to an OpenCode ACP process and its reply.
async fn acp_call(
    input: &mut tokio::process::ChildStdin,
    lines: &mut Lines,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    use tokio::io::AsyncWriteExt;
    let mut bytes =
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .map_err(|_| "Cannot encode OpenCode request")?;
    bytes.push(b'\n');
    input
        .write_all(&bytes)
        .await
        .map_err(|_| "OpenCode closed its input")?;
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|_| "Cannot read OpenCode output")?
            .ok_or("OpenCode exited before it answered")?;
        if line.len() > 16 * 1024 * 1024 {
            return Err("OpenCode emitted an oversized protocol line".into());
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value["id"].as_u64() == Some(id) && value.get("method").is_none() {
            if value.get("error").is_some() {
                return Err(format!("OpenCode refused {method}"));
            }
            return Ok(value["result"].clone());
        }
    }
}

/// OpenCode sessions of every directory, newest first, from its ACP
/// `session/list`. `directory` is only the process's working directory.
pub(super) async fn opencode_list(
    executable: &Path,
    directory: &Path,
    query: &str,
    limit: usize,
    panel_root: &Path,
) -> Result<Vec<Session>, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut child = opencode_command(executable, directory)
        .arg("acp")
        .spawn()
        .map_err(|_| "Could not start OpenCode to list its sessions")?;
    let mut input = child.stdin.take().ok_or("OpenCode input is unavailable")?;
    let mut lines = BufReader::new(
        child
            .stdout
            .take()
            .ok_or("OpenCode output is unavailable")?,
    )
    .lines();
    let work = async {
        let init = acp_call(
            &mut input,
            &mut lines,
            1,
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{}}),
        )
        .await?;
        if init["agentCapabilities"]["sessionCapabilities"]
            .get("list")
            .is_none()
        {
            return Err(
                "This OpenCode version cannot list its sessions; update OpenCode".to_string(),
            );
        }
        let mut sessions = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..10u64 {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |c| json!({"cursor":c}));
            let result = acp_call(&mut input, &mut lines, page + 2, "session/list", params).await?;
            for entry in result["sessions"].as_array().into_iter().flatten() {
                let Some(session) = opencode_session(entry) else {
                    continue;
                };
                if session
                    .cwd
                    .as_ref()
                    .is_some_and(|c| c.starts_with(panel_root))
                    || !matches(&session, query)
                {
                    continue;
                }
                sessions.push(session);
            }
            cursor = result["nextCursor"]
                .as_str()
                .filter(|c| c.len() <= 4096)
                .map(String::from);
            if sessions.len() >= limit || cursor.is_none() {
                break;
            }
        }
        sessions.truncate(limit);
        Ok(sessions)
    };
    let result = tokio::time::timeout(Duration::from_secs(20), work)
        .await
        .map_err(|_| "OpenCode did not list its sessions in time".to_string());
    let _ = child.start_kill();
    let _ = child.wait().await;
    result?
}

/// `opencode session export <id>`, bounded.
pub(super) async fn opencode_export(
    executable: &Path,
    directory: &Path,
    id: &str,
) -> Result<Value, String> {
    use tokio::io::AsyncReadExt;
    if !valid_id("opencode", id) {
        return Err("Invalid session id for this agent".into());
    }
    let mut child = opencode_command(executable, directory)
        .args(["session", "export", "--standalone", id])
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|_| "Could not start OpenCode to read the session")?;
    let mut output = child
        .stdout
        .take()
        .ok_or("OpenCode output is unavailable")?;
    let read = async {
        let mut bytes = Vec::new();
        (&mut output)
            .take(MAX_EXPORT as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Cannot read OpenCode's session export")?;
        Ok::<_, String>(bytes)
    };
    let bytes = tokio::time::timeout(Duration::from_secs(30), read)
        .await
        .map_err(|_| "OpenCode did not export the session in time".to_string())??;
    let _ = child.start_kill();
    let _ = child.wait().await;
    if bytes.len() > MAX_EXPORT {
        return Err("The OpenCode session is too large to import".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "OpenCode's session export was not readable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/agent_panel/fixtures/resume"
    );

    fn fixture(name: &str) -> String {
        fs::read_to_string(Path::new(FIXTURES).join(name)).unwrap()
    }

    struct Home {
        dir: tempfile::TempDir,
        roots: Roots,
    }
    fn home() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let roots = Roots {
            claude: dir.path().join(".claude"),
            codex: dir.path().join(".codex"),
            pi: dir.path().join(".pi/agent/sessions"),
        };
        Home { dir, roots }
    }
    fn put(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    const CLAUDE_ID: &str = "49aa0704-6349-4d60-80f9-9d691cbe67fc";
    const CODEX_ID: &str = "01a1400d-f2f4-7563-85af-3b53be3105ce";
    const PI_ID: &str = "01a1400a-e900-74c1-b099-aff379e76663";

    fn claude_file(h: &Home) -> PathBuf {
        h.roots
            .claude
            .join("projects/-WORK-claude-a")
            .join(format!("{CLAUDE_ID}.jsonl"))
    }
    fn codex_file(h: &Home) -> PathBuf {
        h.roots.codex.join(format!(
            "sessions/2026/10/15/rollout-2026-10-15T23-53-38-{CODEX_ID}.jsonl"
        ))
    }
    fn pi_file(h: &Home) -> PathBuf {
        h.roots
            .pi
            .join("--WORK-pi-a--")
            .join(format!("2026-10-15T14-50-19-265Z_{PI_ID}.jsonl"))
    }

    #[test]
    fn ids_are_canonical_and_cannot_name_paths() {
        assert!(valid_id("claude", CLAUDE_ID));
        assert!(valid_id("opencode", "ses_ebfef45e6ffer5XeO5YfrzRn5s"));
        for bad in [
            "../etc/passwd",
            "49AA0704-6349-4d60-80f9-9d691cbe67fc",
            "49aa07046349 4d6080f99d691cbe67fc",
            "{49aa0704-6349-4d60-80f9-9d691cbe67fc}",
            "",
        ] {
            assert!(!valid_id("claude", bad), "{bad}");
        }
        for bad in [
            "ses_",
            "ses_a/b",
            "ses_../x",
            "sess_abc",
            &format!("ses_{}", "a".repeat(80)),
        ] {
            assert!(!valid_id("opencode", bad), "{bad}");
        }
        assert_eq!(
            file_id(
                "codex",
                &format!("rollout-2026-10-15T23-53-38-{CODEX_ID}.jsonl")
            )
            .as_deref(),
            Some(CODEX_ID)
        );
        assert_eq!(
            file_id("pi", &format!("2026-10-15T14-50-19-265Z_{PI_ID}.jsonl")).as_deref(),
            Some(PI_ID)
        );
        assert_eq!(file_id("claude", "agent-a1b2.jsonl"), None);
    }

    #[test]
    fn claude_fixture_lists_and_imports() {
        let h = home();
        put(&claude_file(&h), &fixture("claude-session.jsonl"));
        let listed = list_files("claude", &h.roots, "", 10, Path::new("/panel"));
        assert_eq!(listed.len(), 1);
        let s = &listed[0];
        assert_eq!(s.id, CLAUDE_ID);
        assert_eq!(s.cwd.as_deref(), Some(Path::new("/WORK/claude-a")));
        assert_eq!(
            s.title.as_deref(),
            Some("Remember the codeword PELICAN-42. Reply with just OK.")
        );
        assert!(s.running, "a file written seconds ago counts as running");
        let history = file_history(&find_file("claude", &h.roots, CLAUDE_ID).unwrap());
        let texts: Vec<_> = history
            .turns
            .iter()
            .map(|t| (t.role, t.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (
                    "user",
                    "Remember the codeword PELICAN-42. Reply with just OK."
                ),
                ("assistant", "OK"),
                ("user", "What codeword?"),
                ("assistant", "PELICAN-42"),
            ]
        );
        assert!(!history.partial);
        assert_eq!(history.unparsed, 0);
    }

    #[test]
    fn claude_title_prefers_custom_title_and_skips_injected_text() {
        let h = home();
        let lines = [
            json!({"type":"user","isMeta":true,"cwd":"/p","message":{"role":"user","content":"meta"}}),
            json!({"type":"user","cwd":"/p","message":{"role":"user","content":"<command-name>/clear</command-name>"}}),
            json!({"type":"user","cwd":"/p","message":{"role":"user","content":[{"type":"text","text":"Fix the login bug"}]}}),
            json!({"type":"assistant","cwd":"/p","message":{"id":"m1","role":"assistant","content":[{"type":"thinking","thinking":"hidden"},{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"/p/a"}},{"type":"tool_use","id":"t2","name":"Bash","input":{}}]}}),
            json!({"type":"user","cwd":"/p","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"file text"}]}}),
            json!({"type":"assistant","isSidechain":true,"cwd":"/p","message":{"role":"assistant","content":[{"type":"text","text":"subagent"}]}}),
            json!({"type":"assistant","cwd":"/p","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Done."}]}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        put(&claude_file(&h), &text);
        let s = &list_files("claude", &h.roots, "", 10, Path::new("/panel"))[0];
        assert_eq!(s.title.as_deref(), Some("Fix the login bug"));
        let turns = file_history(s).turns;
        assert_eq!(
            turns,
            vec![
                Turn {
                    role: "user",
                    text: "Fix the login bug".into()
                },
                Turn {
                    role: "tool",
                    text: "Read, Bash".into()
                },
                Turn {
                    role: "assistant",
                    text: "Done.".into()
                },
            ]
        );
        let mut named = text.clone();
        named.push_str(&format!(
            "{}\n",
            json!({"type":"custom-title","customTitle":"Login bug","sessionId":CLAUDE_ID})
        ));
        put(&claude_file(&h), &named);
        assert_eq!(
            list_files("claude", &h.roots, "", 10, Path::new("/panel"))[0]
                .title
                .as_deref(),
            Some("Login bug")
        );
    }

    #[test]
    fn codex_fixture_lists_and_imports_but_skips_panel_threads() {
        let h = home();
        put(&codex_file(&h), &fixture("codex-session.jsonl"));
        let s = &list_files("codex", &h.roots, "", 10, Path::new("/panel"))[0];
        assert_eq!(s.id, CODEX_ID);
        assert_eq!(s.cwd.as_deref(), Some(Path::new("/WORK/a")));
        assert_eq!(
            s.model.as_deref(),
            Some("gpt-6.1-sol"),
            "the latest turn's model"
        );
        assert_eq!(
            s.title.as_deref(),
            Some("Remember the codeword OSPREY-19. Reply with just OK.")
        );
        let turns = file_history(&find_file("codex", &h.roots, CODEX_ID).unwrap()).turns;
        assert_eq!(
            turns.iter().filter(|t| t.role == "user").count(),
            2,
            "environment context is not a user message"
        );
        assert_eq!(
            turns.last().unwrap(),
            &Turn {
                role: "assistant",
                text: "OSPREY-19".into()
            }
        );
        // The thread index names threads.
        put(
            &h.roots.codex.join("session_index.jsonl"),
            &format!("{}\n", json!({"id":CODEX_ID,"thread_name":"Codeword test"})),
        );
        assert_eq!(
            list_files("codex", &h.roots, "", 10, Path::new("/panel"))[0]
                .title
                .as_deref(),
            Some("Codeword test")
        );
        // A thread the panel started is a panel conversation already.
        let panel = fixture("codex-session.jsonl").replacen(
            "\"cwd\":\"/WORK/a\"",
            "\"cwd\":\"/WORK/a\",\"originator\":\"chrome_use_panel\"",
            1,
        );
        let panel = panel.replacen("\"originator\":\"codex_exec\",", "", 1);
        put(&codex_file(&h), &panel);
        assert!(list_files("codex", &h.roots, "", 10, Path::new("/panel")).is_empty());
    }

    #[test]
    fn pi_fixture_lists_and_imports() {
        let h = home();
        let mut text = fixture("pi-session.jsonl");
        text.push_str(&format!("{}\n", json!({"type":"message","id":"x1","parentId":null,"timestamp":"2026-10-15T15:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"HERON-77"},{"type":"toolCall","id":"c","name":"read","arguments":{}}]}})));
        put(&pi_file(&h), &text);
        let s = &list_files("pi", &h.roots, "heron", 10, Path::new("/panel"))[0];
        assert_eq!(s.id, PI_ID);
        assert_eq!(s.cwd.as_deref(), Some(Path::new("/WORK/pi-a")));
        let turns = file_history(&find_file("pi", &h.roots, PI_ID).unwrap()).turns;
        assert_eq!(
            turns,
            vec![
                Turn {
                    role: "user",
                    text: "Remember the codeword HERON-77. Reply with just OK.".into()
                },
                Turn {
                    role: "user",
                    text: "What codeword?".into()
                },
                Turn {
                    role: "assistant",
                    text: "HERON-77".into()
                },
                Turn {
                    role: "tool",
                    text: "read".into()
                },
            ],
            "failed answers without content are not messages"
        );
        assert!(list_files("pi", &h.roots, "no such words", 10, Path::new("/panel")).is_empty());
    }

    #[test]
    fn opencode_fixture_imports_and_lists() {
        let export: Value = serde_json::from_str(&fixture("opencode-export.json")).unwrap();
        let turns = opencode_history(&export).turns;
        assert_eq!(turns.first().unwrap().role, "user");
        assert_eq!(
            turns.last().unwrap(),
            &Turn {
                role: "assistant",
                text: "EGRET-55".into()
            }
        );
        let entry = json!({"sessionId":"ses_ebfef45e6ffer5XeO5YfrzRn5s","cwd":"/WORK/od","title":"Codeword memory test","updatedAt":"2026-10-15T14:57:16.615Z"});
        let s = opencode_session(&entry).unwrap();
        assert_eq!(s.last_active, 1792076236615);
        assert!(!s.running);
        assert!(opencode_session(&json!({"sessionId":"../x"})).is_none());
        assert_eq!(opencode_history(&json!({"nope":1})).unparsed, 1);
        let panel_turn = json!({"messages":[{"type":"user","text":format!("{}\n\nWhat codeword?", super::super::backend::PROMPT)}]});
        assert_eq!(
            opencode_history(&panel_turn).turns,
            vec![Turn {
                role: "user",
                text: "What codeword?".into()
            }],
            "the panel's instructions are not shown as the user's text"
        );
    }

    #[test]
    fn panel_sessions_are_left_out() {
        let h = home();
        let fixture = fixture("claude-session.jsonl")
            .replace("/WORK/claude-a", "/panel/conversations/x/workspace");
        put(&claude_file(&h), &fixture);
        assert!(list_files("claude", &h.roots, "", 10, Path::new("/panel")).is_empty());
    }

    #[test]
    fn lookup_rejects_bad_ids_and_paths_outside_the_root() {
        let h = home();
        assert!(find_file("claude", &h.roots, "../../etc/passwd").is_err());
        assert!(find_file("claude", &h.roots, CLAUDE_ID).is_err(), "missing");
        assert!(
            find_file("opencode", &h.roots, "ses_x").is_err(),
            "not file based"
        );
        #[cfg(unix)]
        {
            // A symlinked project folder pointing elsewhere is not followed.
            let outside = h.dir.path().join("outside");
            put(
                &outside.join(format!("{CLAUDE_ID}.jsonl")),
                &fixture("claude-session.jsonl"),
            );
            fs::create_dir_all(h.roots.claude.join("projects")).unwrap();
            std::os::unix::fs::symlink(&outside, h.roots.claude.join("projects/-linked")).unwrap();
            assert!(find_file("claude", &h.roots, CLAUDE_ID).is_err());
            assert!(list_files("claude", &h.roots, "", 10, Path::new("/panel")).is_empty());
        }
    }

    #[test]
    fn history_is_bounded_and_large_files_are_read_from_the_end() {
        let h = home();
        let mut text = String::new();
        // Enough text that only the end of the file is read.
        for i in 0..900 {
            text.push_str(&format!("{}\n", json!({"type":"user","cwd":"/p","message":{"role":"user","content":format!("question {i} {}", "x".repeat(3000))}})));
        }
        text.push_str("{not json\n");
        put(&claude_file(&h), &text);
        let history = file_history(&find_file("claude", &h.roots, CLAUDE_ID).unwrap());
        assert!(history.partial);
        assert_eq!(history.unparsed, 1);
        assert!(history.turns.len() <= MAX_IMPORTED);
        assert!(history.omitted > 0);
        let total: usize = history.turns.iter().map(|t| t.text.chars().count()).sum();
        assert!(total <= MAX_IMPORTED_CHARS);
        assert!(history
            .turns
            .last()
            .unwrap()
            .text
            .starts_with("question 899"));
    }

    #[test]
    fn running_claude_sessions_are_found_in_the_live_registry() {
        let h = home();
        put(&claude_file(&h), &fixture("claude-session.jsonl"));
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(claude_file(&h))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(!list_files("claude", &h.roots, "", 10, Path::new("/panel"))[0].running);
        put(
            &h.roots.claude.join("sessions/1.json"),
            &json!({"pid":std::process::id(),"sessionId":CLAUDE_ID}).to_string(),
        );
        assert!(list_files("claude", &h.roots, "", 10, Path::new("/panel"))[0].running);
    }

    #[test]
    fn roots_follow_the_cli_overrides() {
        let mut context = DiscoveryContext::current().unwrap();
        let x = std::env::temp_dir().join("x");
        context
            .environment
            .insert("CODEX_HOME".into(), x.join("codex").into_os_string());
        context.environment.insert(
            "CLAUDE_CONFIG_DIR".into(),
            x.join("claude").into_os_string(),
        );
        context
            .environment
            .insert("PI_CODING_AGENT_DIR".into(), x.join("pi").into_os_string());
        context.environment.remove("PI_CODING_AGENT_SESSION_DIR");
        let roots = Roots::from_context(&context);
        assert_eq!(roots.codex, x.join("codex"));
        assert_eq!(roots.claude, x.join("claude"));
        assert_eq!(roots.pi, x.join("pi").join("sessions"));
    }
}
