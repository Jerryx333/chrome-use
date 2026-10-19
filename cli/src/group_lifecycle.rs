//! What happens to a session's tab group once its task is over.
//!
//! Every agent session has its own ✦ tab group in the user's window. The
//! session name is keyed on the agent's id, so every new conversation gets a
//! new group. The daemon's idle exit leaves its tabs open on purpose (their
//! page state may still be wanted), and only an explicit `close` or `session
//! stop` ever closed them. Agents rarely ran either, so groups piled up in
//! the user's window by the dozen. Three things end that:
//!
//! - **Reuse.** A new session from the same terminal and working directory
//!   takes over the tab group of the one before it, once that one's daemon
//!   has exited ([`inherit_predecessor`]). Its tabs become the new session's
//!   tabs and new tabs join the same group.
//! - **Sweep.** Sessions whose daemon has exited and that saw no command for
//!   [`cleanup_idle`] get their tabs closed ([`sweep`]). A tab the user is
//!   looking at, or has switched to, is left for them instead: ungrouped and
//!   recorded as kept. Every new daemon starts one in the background
//!   ([`maybe_spawn_auto_sweep`]); `tabs cleanup` runs one at once.
//! - **`done`**, in the daemon: the agent ends its task, keeping what the user
//!   needs and closing the rest (see `handle_done` in actions.rs).
//!
//! Deletion rights still come only from a session's created-targets record,
//! closed through the same verified path as `session stop`. A tab the agent
//! adopted, the user lent or a session took with `--force` is never in one.
//! Groups with no record behind them are closed only tab by tab through the
//! extension, whose ownership gate refuses any tab an agent did not create.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::connection::get_socket_dir;
use crate::native::cdp::client::CdpClient;

/// Milliseconds a session must have been idle, with its daemon gone, before
/// the automatic sweep closes its tabs. `0` turns the automatic sweep off.
pub const CLEANUP_ENV: &str = "AGENT_BROWSER_GROUP_CLEANUP_MS";
pub const DEFAULT_CLEANUP_MS: u64 = 30 * 60 * 1000;
/// The automatic sweep runs at most this often per machine.
const AUTO_SWEEP_EVERY: Duration = Duration::from_secs(5 * 60);
/// The activity stamp is rewritten at most this often per daemon.
const ACTIVITY_TOUCH_EVERY: Duration = Duration::from_secs(30);
/// How far past the session's last activity a tab's `lastAccessed` must be to
/// count as the user's visit: the stamp can lag a command by
/// [`ACTIVITY_TOUCH_EVERY`], and the agent's own tab creation and activation
/// happen during commands.
const LAST_ACCESSED_MARGIN: Duration = Duration::from_secs(60);
/// Bound on each extension call the sweep makes.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// Why a tab the sweep left for the user is recorded as kept.
pub const USER_KEPT_REASON: &str = "user";

/// The idle time after which the automatic sweep closes an exited session's
/// tabs, or `None` when it is turned off.
pub fn cleanup_idle() -> Option<Duration> {
    cleanup_idle_from(std::env::var(CLEANUP_ENV).ok().as_deref())
}

fn cleanup_idle_from(raw: Option<&str>) -> Option<Duration> {
    match raw.map(str::trim) {
        None | Some("") => Some(Duration::from_millis(DEFAULT_CLEANUP_MS)),
        Some(raw) => match raw.parse::<u64>() {
            Ok(0) => None,
            Ok(ms) => Some(Duration::from_millis(ms)),
            // A typo must not silently turn cleanup off.
            Err(_) => Some(Duration::from_millis(DEFAULT_CLEANUP_MS)),
        },
    }
}

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

fn activity_path(session: &str) -> PathBuf {
    get_socket_dir().join(format!("{session}.activity"))
}

/// Record that `session` just ran a command. Throttled: the sweep measures
/// idleness in tens of minutes, so one write per half minute is plenty.
pub fn touch_activity(session: &str) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if let Ok(mut last) = LAST.lock() {
        if last.is_some_and(|at| at.elapsed() < ACTIVITY_TOUCH_EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    let _ = std::fs::write(activity_path(session), now_ms().to_string());
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// When `session` last did anything: its activity stamp, or for a session from
/// a build before the stamp existed, the last change to its tab record.
pub fn last_activity(session: &str) -> Option<SystemTime> {
    [
        activity_path(session),
        crate::connection::created_targets_path(session),
    ]
    .iter()
    .filter_map(|p| modified(p))
    .max()
}

fn now_ms() -> u64 {
    millis(SystemTime::now())
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Lineage: which earlier session a new one continues
// ---------------------------------------------------------------------------

/// Terminal ids, finest first: a tmux pane inside a terminal tab is the more
/// specific of the two. One of these plus the working directory says "the
/// same place the user works in", which is what a follow-up task shares with
/// the one before it. Agent ids are left out on purpose: they are what
/// changes between those two tasks.
const LINEAGE_TERMINAL_VARS: &[&str] = &[
    "CMUX_SURFACE_ID",
    "TMUX_PANE",
    "STY",
    "WEZTERM_PANE",
    "KITTY_WINDOW_ID",
    "GHOSTTY_SURFACE_ID",
    "ITERM_SESSION_ID",
    "TERM_SESSION_ID",
    "WT_SESSION",
    "WINDOWID",
    "SHELL_SESSION_ID",
];

/// The lineage key of an agent working in `cwd` with environment `vars`.
/// Without a terminal id (an agent app with no terminal) it is the directory
/// alone.
pub fn lineage_of(cwd: &Path, vars: &[(String, String)]) -> String {
    let terminal = LINEAGE_TERMINAL_VARS.iter().find_map(|name| {
        vars.iter()
            .find(|(k, v)| k == name && !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
    });
    let mut hasher = Sha256::new();
    hasher.update(cwd.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(terminal.unwrap_or_default().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    digest[..16].to_string()
}

fn current_lineage() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    Some(lineage_of(&cwd, &std::env::vars().collect::<Vec<_>>()))
}

fn lineage_path(session: &str) -> PathBuf {
    get_socket_dir().join(format!("{session}.lineage"))
}

fn read_lineage(session: &str) -> Option<String> {
    std::fs::read_to_string(lineage_path(session))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn moved_path(session: &str) -> PathBuf {
    get_socket_dir().join(format!("{session}.moved-to"))
}

/// An earlier session as [`pick_predecessor`] sees it.
#[derive(Debug, Clone)]
pub struct PriorSession {
    pub name: String,
    pub lineage: Option<String>,
    pub live: bool,
    pub has_record: bool,
    pub last_activity: Option<SystemTime>,
}

/// The session whose tab group `own` should continue: same lineage, daemon
/// gone, tabs still recorded; the most recently active of them.
pub fn pick_predecessor<'a>(
    own: &str,
    lineage: &str,
    sessions: &'a [PriorSession],
) -> Option<&'a PriorSession> {
    sessions
        .iter()
        .filter(|s| {
            s.name != own && !s.live && s.has_record && s.lineage.as_deref() == Some(lineage)
        })
        .max_by(|a, b| {
            a.last_activity
                .cmp(&b.last_activity)
                .then_with(|| a.name.cmp(&b.name))
        })
}

fn prior_sessions() -> Vec<PriorSession> {
    let Ok(entries) = std::fs::read_dir(get_socket_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".lineage")
                .map(str::to_string)
        })
        .map(|name| PriorSession {
            lineage: read_lineage(&name),
            live: crate::connection::daemon_ready(&name),
            has_record: crate::connection::has_created_targets(&name),
            last_activity: last_activity(&name),
            name,
        })
        .collect()
}

/// Called before a derived (not `--session`) session's daemon starts. Records
/// where this session runs, and when it is starting from nothing — no record,
/// no title — takes over the most recent exited session from the same
/// terminal and directory: its tab record moves here and its group label
/// with it, so the relay attributes its tabs to this session and new tabs
/// join the same group. Returns the session taken over.
pub fn inherit_predecessor(session: &str) -> Option<String> {
    let lineage = current_lineage()?;
    if read_lineage(session).is_none() {
        let _ = std::fs::write(lineage_path(session), &lineage);
    }
    if crate::connection::has_created_targets(session)
        || crate::session_title::title_of(session).is_some()
    {
        return None;
    }
    let sessions = prior_sessions();
    let prior = pick_predecessor(session, &lineage, &sessions)?.name.clone();
    // Never wait here: a lock held means that session is starting again.
    let _lock = crate::connection::try_lock_session_lifecycle(&prior)?;
    if crate::connection::daemon_ready(&prior) || !crate::connection::has_created_targets(&prior) {
        return None;
    }
    let label = crate::session_title::title_of(&prior).unwrap_or_else(|| prior.clone());
    crate::connection::move_created_targets(&prior, session).ok()?;
    if crate::session_title::set_title(session, &label).is_err() {
        // Without the label the moved tabs would sit in a group this session
        // does not own; give them back.
        let _ = crate::connection::move_created_targets(session, &prior);
        return None;
    }
    crate::session_title::clear_title(&prior);
    let _ = std::fs::write(moved_path(&prior), session);
    Some(prior)
}

/// When this session's tabs were taken over by a later one, which one; the
/// note is given once.
pub fn take_moved_note(session: &str) -> Option<String> {
    let path = moved_path(session);
    let to = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(to.trim().to_string()).filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Sweep
// ---------------------------------------------------------------------------

/// What one sweep may touch.
#[derive(Debug, Clone, Copy)]
pub struct SweepOptions {
    /// Only sessions idle at least this long; `None` for any exited session.
    pub idle: Option<Duration>,
    /// Also the groups no record explains: under a ✦ title, and the groups
    /// of older builds (⬉, a trailing 🤖, a bare session id).
    pub orphans: bool,
}

/// How a group title marks an agent's group, if it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentGroupTitle {
    /// `✦ <label>`, the current mark.
    Current(String),
    /// From a build before ✦: `⬉ <label>`, `<label> 🤖` or a bare session id.
    Legacy(String),
}

/// Whether a tab group title is one chrome-use gave a session's group.
pub fn agent_group_title(title: &str) -> Option<AgentGroupTitle> {
    let title = title.trim();
    if let Some(rest) = title.strip_prefix(crate::session_title::GROUP_MARK) {
        return Some(AgentGroupTitle::Current(rest.trim().to_string()));
    }
    if let Some(rest) = title.strip_prefix('⬉') {
        return Some(AgentGroupTitle::Legacy(rest.trim().to_string()));
    }
    if let Some(rest) = title.strip_suffix('🤖') {
        return Some(AgentGroupTitle::Legacy(rest.trim().to_string()));
    }
    if looks_like_session_id(title) {
        return Some(AgentGroupTitle::Legacy(title.to_string()));
    }
    None
}

/// A derived session name, `cu-<repo>-<6 hex>` or `cu-<6 hex>`.
fn looks_like_session_id(title: &str) -> bool {
    let Some(rest) = title.strip_prefix("cu-") else {
        return false;
    };
    let tag = rest.rsplit('-').next().unwrap_or("");
    tag.len() == 6
        && tag.chars().all(|c| c.is_ascii_hexdigit())
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// One Chrome tab as the sweep judges it.
#[derive(Debug, Clone, Default)]
pub struct TabFacts {
    pub id: i64,
    /// In front of its window: the user may be reading or typing in it.
    pub active: bool,
    /// Chrome's `lastAccessed`, when it reports one.
    pub last_accessed_ms: Option<u64>,
    /// The extension saw the user switch to it (0.5.77+).
    pub touched: bool,
}

impl TabFacts {
    fn from_tab(tab: &Value, touched: &HashSet<i64>) -> Option<Self> {
        let id = tab.get("id")?.as_i64()?;
        Some(TabFacts {
            id,
            active: tab.get("active").and_then(Value::as_bool).unwrap_or(false),
            last_accessed_ms: tab
                .get("lastAccessed")
                .and_then(Value::as_f64)
                .map(|ms| ms as u64),
            touched: touched.contains(&id),
        })
    }
}

/// Whether the user has this tab: in front of a window now, switched to by
/// the user (as the extension saw it), or — for an extension too old to say —
/// accessed well after the session's last command, which the agent never
/// does.
pub fn user_has_tab(tab: &TabFacts, session_last_activity: Option<SystemTime>) -> bool {
    if tab.active || tab.touched {
        return true;
    }
    match (tab.last_accessed_ms, session_last_activity) {
        (Some(accessed), Some(last)) => {
            accessed > millis(last) + LAST_ACCESSED_MARGIN.as_millis() as u64
        }
        _ => false,
    }
}

/// Whether a session last active at `last` is idle enough for `idle`.
pub fn idle_enough(last: Option<SystemTime>, idle: Option<Duration>, now: SystemTime) -> bool {
    match (idle, last) {
        (None, _) => true,
        // No trace of activity at all: old enough by any measure.
        (Some(_), None) => true,
        (Some(idle), Some(last)) => now.duration_since(last).is_ok_and(|age| age >= idle),
    }
}

/// What a sweep did, for `tabs cleanup` and the log.
#[derive(Debug, Default)]
pub struct SweepReport {
    pub closed: Vec<Value>,
    pub left_for_user: Vec<Value>,
    pub skipped: Vec<Value>,
    pub errors: Vec<String>,
}

impl SweepReport {
    pub fn to_json(&self) -> Value {
        json!({
            "closed": self.closed,
            "leftForUser": self.left_for_user,
            "skipped": self.skipped,
            "errors": self.errors,
        })
    }
}

/// What the extension tells the sweep, read once.
struct Browser {
    client: CdpClient,
    tabs: HashMap<i64, Value>,
    groups: Vec<Value>,
    /// target id -> Chrome tab id, for tabs the extension holds attached.
    tab_of_target: HashMap<String, i64>,
    /// Chrome tab ids the extension created for an agent.
    owned: HashSet<i64>,
    touched: HashSet<i64>,
}

async fn call(client: &CdpClient, method: &str, params: Value) -> Result<Value, String> {
    tokio::time::timeout(
        CALL_TIMEOUT,
        client.send_command(method, Some(params), None),
    )
    .await
    .map_err(|_| format!("{method} did not answer"))?
}

async fn chrome(
    client: &CdpClient,
    namespace: &str,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let out = call(
        client,
        "ABExt.call",
        json!({ "namespace": namespace, "method": method, "args": args }),
    )
    .await?;
    Ok(out.get("result").cloned().unwrap_or(Value::Null))
}

impl Browser {
    async fn read(endpoint: &str) -> Result<Self, String> {
        // A passive connection: the native host does not have the extension
        // re-attach and re-announce every agent tab for it.
        let client = CdpClient::connect_with_headers(
            endpoint,
            Some(vec![("x-chrome-use-diagnostic".into(), "1".into())]),
        )
        .await?;
        let tabs: HashMap<i64, Value> = chrome(&client, "tabs", "query", json!([{}]))
            .await?
            .as_array()
            .ok_or("chrome.tabs.query returned no list")?
            .iter()
            .filter_map(|t| Some((t.get("id")?.as_i64()?, t.clone())))
            .collect();
        let groups = chrome(&client, "tabGroups", "query", json!([{}]))
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default();
        // Without it (an old extension) nothing is known to be an agent's
        // tab, and only recorded tabs are judged.
        let state = call(&client, "ABExt.state", json!({}))
            .await
            .unwrap_or(Value::Null);
        let ids = |key: &str| -> HashSet<i64> {
            state
                .get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default()
        };
        let tab_of_target = state
            .get("attachedTargets")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        Some((
                            r.get("targetId")?.as_str()?.to_string(),
                            r.get("tabId")?.as_i64()?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Browser {
            owned: ids("ownedTabs"),
            touched: ids("userTouchedTabs"),
            client,
            tabs,
            groups,
            tab_of_target,
        })
    }

    fn facts(&self, tab_id: i64) -> Option<TabFacts> {
        TabFacts::from_tab(self.tabs.get(&tab_id)?, &self.touched)
    }

    fn row(&self, tab_id: i64, session: Option<&str>) -> Value {
        let tab = self.tabs.get(&tab_id);
        json!({
            "tab": tab_id,
            "session": session,
            "title": tab.and_then(|t| t.get("title")).cloned().unwrap_or(Value::Null),
        })
    }

    /// Leave a tab to the user: out of the agent's group. Best effort; an
    /// ungroup the extension refuses leaves the tab where it is, still open.
    async fn ungroup(&self, tab_id: i64) {
        let _ = chrome(&self.client, "tabs", "ungroup", json!([tab_id])).await;
    }
}

fn recorded_sessions() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(get_socket_dir()) else {
        return Vec::new();
    };
    let mut sessions: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(crate::connection::CREATED_TARGETS_SUFFIX)
                .map(str::to_string)
        })
        .collect();
    sessions.sort();
    sessions
}

/// Close the tabs of exited, idle sessions on the browser at `endpoint` (an
/// extension relay), leaving the ones the user has to them, then the agent
/// groups no record explains. Never touches a session whose daemon is
/// running.
pub async fn sweep(endpoint: &str, opts: SweepOptions) -> Result<SweepReport, String> {
    if !crate::connect::is_relay_url(endpoint) {
        return Err(
            "tab groups exist only in Chrome through the chrome-use extension; this browser \
             is not connected that way"
                .to_string(),
        );
    }
    let browser = Browser::read(endpoint).await?;
    let mut report = SweepReport::default();
    let now = SystemTime::now();

    for session in recorded_sessions() {
        sweep_session(&browser, endpoint, &session, opts, now, &mut report).await;
    }
    sweep_unrecorded_groups(&browser, opts, &mut report).await;
    Ok(report)
}

async fn sweep_session(
    browser: &Browser,
    endpoint: &str,
    session: &str,
    opts: SweepOptions,
    now: SystemTime,
    report: &mut SweepReport,
) {
    let skip = |report: &mut SweepReport, why: &str| {
        report
            .skipped
            .push(json!({ "session": session, "reason": why }));
    };
    let Some(_lock) = crate::connection::try_lock_session_lifecycle(session) else {
        return skip(report, "starting or stopping right now");
    };
    if crate::connection::daemon_ready(session) {
        return skip(report, "running");
    }
    let last = last_activity(session);
    if !idle_enough(last, opts.idle, now) {
        return skip(report, "used recently");
    }
    let targets = crate::connection::read_created_targets(session, endpoint);
    if targets.is_empty() {
        // Recorded in another browser or profile; not this sweep's to judge.
        return;
    }
    let mut close = HashSet::new();
    for target in targets {
        let facts = browser
            .tab_of_target
            .get(&target)
            .and_then(|tab| browser.facts(*tab));
        match facts {
            Some(tab) if user_has_tab(&tab, last) => {
                browser.ungroup(tab.id).await;
                let _ = crate::connection::set_kept_tab(session, &target, USER_KEPT_REASON);
                report
                    .left_for_user
                    .push(browser.row(tab.id, Some(session)));
            }
            _ => {
                close.insert(target);
            }
        }
    }
    // The kept tabs leave the record before anything closes, so a failure
    // below can never take them with it.
    if let Err(e) = crate::connection::write_created_targets(session, endpoint, &close) {
        report.errors.push(format!("session {session}: {e}"));
        return;
    }
    if close.is_empty() {
        return;
    }
    let rows: HashMap<String, Value> = close
        .iter()
        .map(|t| {
            let row = match browser.tab_of_target.get(t) {
                Some(tab) => browser.row(*tab, Some(session)),
                None => json!({ "target": t, "session": session }),
            };
            (t.clone(), row)
        })
        .collect();
    match crate::native::browser::close_persisted_session_tabs_at(session, endpoint).await {
        Ok(closed) => {
            for target in closed {
                if let Some(row) = rows.get(&target) {
                    report.closed.push(row.clone());
                }
            }
        }
        Err(e) => report.errors.push(format!("session {session}: {e}")),
    }
}

/// The labels of sessions whose daemon is running: their groups are in use.
fn live_labels() -> HashSet<String> {
    let mut labels = HashSet::new();
    // Read-only: `walk_daemons` would also clear the files of dead daemons,
    // bindings included, which the sweep must leave for `session stop`.
    let Ok(entries) = std::fs::read_dir(get_socket_dir()) else {
        return labels;
    };
    let names: HashSet<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".sock")
                .or_else(|| name.strip_suffix(".pid"))
                .map(str::to_string)
        })
        .collect();
    for name in names {
        if !crate::connection::daemon_ready(&name) {
            continue;
        }
        let display = crate::session_title::display_name(&name);
        if let Some(AgentGroupTitle::Current(label)) = agent_group_title(&display) {
            labels.insert(label);
        }
        labels.insert(name);
    }
    labels
}

/// Every target some session's record names, as owned or as kept.
fn claimed_targets() -> HashSet<String> {
    let mut claimed = HashSet::new();
    let Ok(entries) = std::fs::read_dir(get_socket_dir()) else {
        return claimed;
    };
    for name in entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
    {
        if let Some(session) = name.strip_suffix(crate::connection::CREATED_TARGETS_SUFFIX) {
            claimed.extend(crate::connection::created_target_ids(session));
        } else if let Some(session) = name.strip_suffix(".kept-tabs.json") {
            claimed.extend(crate::connection::read_kept_tabs(session).into_keys());
        }
    }
    claimed
}

async fn sweep_unrecorded_groups(browser: &Browser, opts: SweepOptions, report: &mut SweepReport) {
    let live = live_labels();
    let claimed = claimed_targets();
    for group in &browser.groups {
        let (Some(group_id), Some(title)) = (
            group.get("id").and_then(Value::as_i64),
            group.get("title").and_then(Value::as_str),
        ) else {
            continue;
        };
        let label = match agent_group_title(title) {
            Some(AgentGroupTitle::Legacy(label) | AgentGroupTitle::Current(label))
                if opts.orphans =>
            {
                label
            }
            _ => continue,
        };
        if live.contains(&label) {
            continue;
        }
        let members: Vec<i64> = browser
            .tabs
            .iter()
            .filter(|(_, t)| t.get("groupId").and_then(Value::as_i64) == Some(group_id))
            .map(|(id, _)| *id)
            .collect();
        for tab_id in members {
            // Only a tab an agent created; a tab some record explains was
            // judged with its session above.
            if !browser.owned.contains(&tab_id) {
                continue;
            }
            let recorded = browser
                .tab_of_target
                .iter()
                .any(|(target, tab)| *tab == tab_id && claimed.contains(target));
            if recorded {
                continue;
            }
            let Some(facts) = browser.facts(tab_id) else {
                continue;
            };
            if user_has_tab(&facts, None) {
                browser.ungroup(tab_id).await;
                report.left_for_user.push(browser.row(tab_id, None));
                continue;
            }
            let row = browser.row(tab_id, None);
            match chrome(&browser.client, "tabs", "remove", json!([tab_id])).await {
                Ok(_) => report.closed.push(row),
                Err(e) => report
                    .errors
                    .push(format!("tab {tab_id} in group {title}: {e}")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Automatic sweep
// ---------------------------------------------------------------------------

fn auto_sweep_stamp() -> PathBuf {
    get_socket_dir().join(".group-sweep")
}

/// Whether an automatic sweep is due, given when the last one started.
pub fn auto_sweep_due(last: Option<SystemTime>, now: SystemTime) -> bool {
    match last {
        None => true,
        Some(last) => now
            .duration_since(last)
            .map_or(true, |age| age >= AUTO_SWEEP_EVERY),
    }
}

/// Whether some session with recorded tabs has no running daemon and has
/// been idle for `idle`: the only thing the automatic sweep acts on. Read from
/// the socket directory alone.
fn sweep_has_work(idle: Option<Duration>) -> bool {
    let now = SystemTime::now();
    recorded_sessions().iter().any(|session| {
        idle_enough(last_activity(session), idle, now) && !crate::connection::daemon_ready(session)
    })
}

/// After a new daemon started for `session`: unless one ran in the last few
/// minutes, sweep in a detached process, so the command that started the
/// daemon never waits for it. The child is a plain `chrome-use tabs cleanup
/// --auto` in this session's binding, which reaches the same Chrome profile.
pub fn maybe_spawn_auto_sweep(session: &str) {
    if cleanup_idle().is_none() {
        return;
    }
    let stamp = auto_sweep_stamp();
    if !auto_sweep_due(modified(&stamp), SystemTime::now()) {
        return;
    }
    // Nothing ended long enough ago: no process, no connection to Chrome.
    if !sweep_has_work(cleanup_idle()) {
        return;
    }
    if std::fs::write(&stamp, now_ms().to_string()).is_err() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--session", session, "tabs", "cleanup", "--auto", "--json"])
        .env_remove("AGENT_BROWSER_DAEMON")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe; the child leaves this process
        // group so the agent's terminal never waits on it.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let _ = cmd.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn cleanup_idle_defaults_and_zero_disables() {
        assert_eq!(
            cleanup_idle_from(None),
            Some(Duration::from_millis(DEFAULT_CLEANUP_MS))
        );
        assert_eq!(cleanup_idle_from(Some("0")), None);
        assert_eq!(
            cleanup_idle_from(Some("60000")),
            Some(Duration::from_secs(60))
        );
        // A typo keeps the default rather than turning cleanup off.
        assert_eq!(
            cleanup_idle_from(Some("30m")),
            Some(Duration::from_millis(DEFAULT_CLEANUP_MS))
        );
    }

    /// The same terminal and directory give the same key whatever the agent
    /// id is; another pane or directory gives another.
    #[test]
    fn lineage_follows_terminal_and_directory_not_agent() {
        let cwd = Path::new("/work/repo");
        let a = lineage_of(cwd, &vars(&[("TMUX_PANE", "%3"), ("CLAUDE_PID", "100")]));
        let b = lineage_of(cwd, &vars(&[("TMUX_PANE", "%3"), ("CLAUDE_PID", "200")]));
        assert_eq!(a, b);
        assert_ne!(a, lineage_of(cwd, &vars(&[("TMUX_PANE", "%4")])));
        assert_ne!(
            a,
            lineage_of(Path::new("/work/other"), &vars(&[("TMUX_PANE", "%3")]))
        );
        // A pane is finer than the terminal tab around it.
        assert_eq!(
            lineage_of(
                cwd,
                &vars(&[("TERM_SESSION_ID", "w0"), ("TMUX_PANE", "%3")])
            ),
            lineage_of(
                cwd,
                &vars(&[("TERM_SESSION_ID", "w1"), ("TMUX_PANE", "%3")])
            )
        );
        // No terminal: the directory alone.
        assert_eq!(lineage_of(cwd, &[]), lineage_of(cwd, &vars(&[("X", "1")])));
    }

    fn prior(name: &str, lineage: &str, live: bool, record: bool, age_s: u64) -> PriorSession {
        PriorSession {
            name: name.to_string(),
            lineage: Some(lineage.to_string()),
            live,
            has_record: record,
            last_activity: Some(SystemTime::now() - Duration::from_secs(age_s)),
        }
    }

    #[test]
    fn predecessor_is_the_latest_exited_session_with_tabs_in_the_same_lineage() {
        let sessions = vec![
            prior("cu-a-old", "L", false, true, 3600),
            prior("cu-a-new", "L", false, true, 60),
            prior("cu-a-live", "L", true, true, 1),
            prior("cu-a-empty", "L", false, false, 1),
            prior("cu-b", "M", false, true, 1),
        ];
        let picked = pick_predecessor("cu-a-me", "L", &sessions).map(|s| s.name.as_str());
        assert_eq!(picked, Some("cu-a-new"));
        // Never itself, and nothing from another lineage.
        assert!(pick_predecessor("cu-b", "M", &sessions[4..]).is_none());
        assert!(pick_predecessor("cu-x", "N", &sessions).is_none());
    }

    #[test]
    fn group_titles_are_recognised_with_every_mark_we_ever_used() {
        assert_eq!(
            agent_group_title("✦ 查快递"),
            Some(AgentGroupTitle::Current("查快递".into()))
        );
        assert_eq!(
            agent_group_title("⬉ cu-repo-3f9a1c"),
            Some(AgentGroupTitle::Legacy("cu-repo-3f9a1c".into()))
        );
        assert_eq!(
            agent_group_title("cu-repo-3f9a1c 🤖"),
            Some(AgentGroupTitle::Legacy("cu-repo-3f9a1c".into()))
        );
        assert_eq!(
            agent_group_title("cu-chrome-use-3f9a1c"),
            Some(AgentGroupTitle::Legacy("cu-chrome-use-3f9a1c".into()))
        );
        // The user's own groups.
        assert_eq!(agent_group_title("Work"), None);
        assert_eq!(agent_group_title("cu-later"), None);
        assert_eq!(agent_group_title("cu-Repo-3f9a1c"), None);
    }

    #[test]
    fn a_tab_in_front_or_switched_to_is_the_users() {
        let last = SystemTime::now() - Duration::from_secs(3600);
        let base = TabFacts {
            id: 1,
            ..Default::default()
        };
        assert!(!user_has_tab(&base, Some(last)));
        assert!(user_has_tab(
            &TabFacts {
                active: true,
                ..base.clone()
            },
            Some(last)
        ));
        assert!(user_has_tab(
            &TabFacts {
                touched: true,
                ..base.clone()
            },
            Some(last)
        ));
        // Accessed long after the agent's last command: the user's visit.
        let visited = millis(last) + 10 * 60 * 1000;
        assert!(user_has_tab(
            &TabFacts {
                last_accessed_ms: Some(visited),
                ..base.clone()
            },
            Some(last)
        ));
        // Within the margin it could be the agent's own creation.
        let created = millis(last) + 20 * 1000;
        assert!(!user_has_tab(
            &TabFacts {
                last_accessed_ms: Some(created),
                ..base.clone()
            },
            Some(last)
        ));
        // Without a session to compare with, lastAccessed proves nothing.
        assert!(!user_has_tab(
            &TabFacts {
                last_accessed_ms: Some(visited),
                ..base
            },
            None
        ));
    }

    #[test]
    fn idleness_is_measured_from_the_last_activity() {
        let now = SystemTime::now();
        let idle = Some(Duration::from_secs(1800));
        assert!(!idle_enough(Some(now - Duration::from_secs(60)), idle, now));
        assert!(idle_enough(
            Some(now - Duration::from_secs(1801)),
            idle,
            now
        ));
        assert!(idle_enough(None, idle, now));
        assert!(idle_enough(Some(now), None, now));
    }

    #[test]
    fn automatic_sweeps_are_spaced_out() {
        let now = SystemTime::now();
        assert!(auto_sweep_due(None, now));
        assert!(!auto_sweep_due(Some(now - Duration::from_secs(10)), now));
        assert!(auto_sweep_due(Some(now - AUTO_SWEEP_EVERY), now));
    }
}
