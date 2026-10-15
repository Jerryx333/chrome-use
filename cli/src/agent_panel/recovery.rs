//! What is known about a backend process whose exit was never confirmed, and
//! the two ways out: end its whole tree (only when ownership is proven) or
//! continue in a new conversation (only when the tree is proven gone).
//!
//! `exited` needs proof that the whole tree is gone, never just the leader.
//! On Unix a live group is called ours only when every member's session is
//! the recorded service session and the OS is known not to reuse a PID that
//! is still a live pgid or sid; anything else is `unknown`, and an `unknown`
//! group is never signalled. On Windows only the held job handle proves
//! anything; after a service restart the answer is `unknown`.
//!
//! This module computes probes and event payloads; the engine journals them.

use super::conversation::Event;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MAX_POSSIBLY_EXECUTED: usize = 20;
const MAX_LAST_TARGETS: usize = 8;
const MAX_ARTIFACTS: usize = 20;
const MAX_URL: usize = 2048;
const MAX_TITLE: usize = 256;
const PREFACE_URL: usize = 200;

/// The backend process a conversation may still have: the last `ready`
/// without a later `exited` (which the backend only sends once the exit was
/// confirmed or the process never started). Fields the `ready` did not record
/// stay `None`, and the probe then answers `unknown`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ProcessRecord {
    pub generation: Option<u64>,
    pub pid: Option<u32>,
    pub pgid: Option<i32>,
    pub process_start_time: Option<u64>,
    pub service_sid: Option<i32>,
}
impl ProcessRecord {
    pub fn from_events<'a>(events: impl IntoIterator<Item = &'a Event>) -> Option<Self> {
        let mut record = None;
        for event in events {
            match event.kind.as_str() {
                "ready" => {
                    let d = &event.data;
                    let int = |key: &str| d[key].as_i64().and_then(|v| i32::try_from(v).ok());
                    record = Some(Self {
                        generation: d["generation"].as_u64(),
                        pid: d["pid"].as_u64().and_then(|v| u32::try_from(v).ok()),
                        pgid: int("pgid").filter(|v| *v > 1),
                        process_start_time: d["processStartTime"].as_u64(),
                        service_sid: int("serviceSid").filter(|v| *v > 1),
                    });
                }
                "exited" => record = None,
                _ => {}
            }
        }
        record
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ProcessState {
    Exited,
    Alive,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Probe {
    pub state: ProcessState,
    pub reason: String,
}
fn probe(state: ProcessState, reason: &str) -> Probe {
    Probe {
        state,
        reason: reason.into(),
    }
}
impl Probe {
    /// `process` block of `conversationRecoveryReview`. Leader PID and start
    /// time are shown for information only.
    pub fn review(&self, record: Option<&ProcessRecord>) -> Value {
        json!({
            "state": self.state,
            "reason": self.reason,
            "pid": record.and_then(|r| r.pid),
            "processStartTime": record.and_then(|r| r.process_start_time),
            "canTerminate": self.state == ProcessState::Alive,
            "canFork": self.state == ProcessState::Exited,
        })
    }
}

/// Whether the running OS is known, from its kernel source, never to hand out
/// a PID that a live process group or session still uses, and never to let a
/// process join a group of another session.
///
/// Linux: `alloc_pid` keeps a `struct pid` alive while any task references it
/// as pgid or sid; `setpgid` refuses a group in another session.
///
/// macOS, read in apple-oss-distributions/xnu at `xnu-12377.121.6` (macOS 26):
/// - `bsd/kern/kern_fork.c:964-974` (`forkproc`): the PID loop skips any
///   candidate for which `phash_find_locked`, `pghash_exists_locked` or
///   `session_find_locked` finds something, so a new process never gets the
///   id of a live process, process group or session. The same loop is in
///   `xnu-11417.140.69` (macOS 15) and `xnu-10063.141.1` (macOS 14).
/// - `bsd/kern/kern_proc.c:3107-3118` (`pgrp_destroy`): a group leaves the
///   hash only when its last reference goes, then releases its session;
///   `kern_proc.c:2849-2873` (`session_destroy`/`session_rele`): a session
///   leaves the hash only when its last group is gone.
/// - `bsd/kern/kern_prot.c:595` and `626` (`setpgid`): moving a process into
///   another group is EPERM unless both are in the caller's session, and
///   `kern_proc.c:2982-3040` (`enterpgrp`) creates a new group only with the
///   caller's own PID as its id.
///
/// So while group P lives, no other process can be P's leader, and a member
/// whose session id is the recorded service session is in that session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ProofPolicy {
    pub pid_allocation_proven: bool,
}
impl ProofPolicy {
    pub fn running_os() -> Self {
        Self {
            pid_allocation_proven: cfg!(any(target_os = "linux", target_os = "macos")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Termination {
    pub method: &'static str,
    pub confirmed: bool,
    pub detail: String,
}
impl Termination {
    pub fn event(&self, message_id: Option<&str>) -> Value {
        json!({
            "v": 1,
            "messageId": message_id,
            "method": self.method,
            "confirmed": self.confirmed,
            "detail": self.detail,
        })
    }
}

#[cfg(unix)]
pub(super) use unix::*;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::time::Duration;

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    /// The session the service created with `setsid`; recorded in `ready`.
    #[cfg_attr(not(test), allow(dead_code))] // backend.rs reads it with its own call.
    pub(in super::super) fn current_session() -> Option<i32> {
        let sid = unsafe { libc::getsid(0) };
        (sid > 1).then_some(sid)
    }

    /// An opaque start-time stamp for `ready.processStartTime`; compare only
    /// with values from this same function. Linux: clock ticks since boot
    /// (`/proc/<pid>/stat` field 22). macOS: microseconds since the epoch.
    pub(in super::super) fn process_start_time(pid: u32) -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            linux_stat(pid as i32).map(|s| s.start_time)
        }
        #[cfg(target_os = "macos")]
        {
            macos_bsdinfo(pid as i32)
                .map(|i| i.pbi_start_tvsec.saturating_mul(1_000_000) + i.pbi_start_tvusec)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = pid;
            None
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) struct LinuxStat {
        pub pgrp: i32,
        pub session: i32,
        pub start_time: u64,
    }
    /// Fields after the last `)`: the command name may hold spaces and parens.
    #[cfg(target_os = "linux")]
    pub(super) fn parse_linux_stat(text: &str) -> Option<LinuxStat> {
        let rest = &text[text.rfind(')')? + 1..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        Some(LinuxStat {
            pgrp: fields.get(2)?.parse().ok()?,
            session: fields.get(3)?.parse().ok()?,
            start_time: fields.get(19)?.parse().ok()?,
        })
    }
    #[cfg(target_os = "linux")]
    fn linux_stat(pid: i32) -> Option<LinuxStat> {
        parse_linux_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    #[cfg(target_os = "macos")]
    fn macos_bsdinfo(pid: i32) -> Option<libc::proc_bsdinfo> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        (n == size).then_some(info)
    }

    enum Group {
        Gone,
        Live,
        Foreign,
    }
    fn group_signal(pgid: i32) -> Group {
        if unsafe { libc::kill(-pgid, 0) } == 0 {
            return Group::Live;
        }
        match errno() {
            libc::ESRCH => Group::Gone,
            _ => Group::Foreign,
        }
    }

    /// Current members of the process group.
    fn members(pgid: i32) -> Result<Vec<i32>, String> {
        #[cfg(target_os = "linux")]
        {
            let entries = std::fs::read_dir("/proc").map_err(|_| "Cannot enumerate processes")?;
            Ok(entries
                .flatten()
                .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
                .filter(|pid| linux_stat(*pid).is_some_and(|s| s.pgrp == pgid))
                .collect())
        }
        #[cfg(target_os = "macos")]
        {
            const PROC_PGRP_ONLY: u32 = 2;
            let count = unsafe {
                libc::proc_listpids(PROC_PGRP_ONLY, pgid as u32, std::ptr::null_mut(), 0)
            };
            if count < 0 {
                return Err("Cannot enumerate the process group".into());
            }
            // Room for members forked since the size query.
            let capacity = count as usize / std::mem::size_of::<i32>() + 64;
            let mut pids = vec![0i32; capacity];
            let written = unsafe {
                libc::proc_listpids(
                    PROC_PGRP_ONLY,
                    pgid as u32,
                    pids.as_mut_ptr() as *mut libc::c_void,
                    (capacity * std::mem::size_of::<i32>()) as i32,
                )
            };
            if written < 0 {
                return Err("Cannot enumerate the process group".into());
            }
            let n = written as usize / std::mem::size_of::<i32>();
            if n >= capacity {
                return Err("The process group grew while it was enumerated".into());
            }
            pids.truncate(n);
            pids.retain(|pid| *pid > 0);
            Ok(pids)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = pgid;
            Err("Process groups cannot be enumerated on this OS".into())
        }
    }

    pub(super) enum Proof {
        Gone,
        Owned(Vec<i32>),
        Unknown(String),
    }

    /// Prove the live group is ours: every member is in the recorded session.
    /// A process cannot join a group in another session, so with PID reuse
    /// excluded these members descend from our backend.
    pub(super) fn prove(pgid: i32, sid: i32, policy: ProofPolicy) -> Proof {
        match group_signal(pgid) {
            Group::Gone => return Proof::Gone,
            Group::Foreign => {
                return Proof::Unknown("The process group belongs to another user".into())
            }
            Group::Live => {}
        }
        if !policy.pid_allocation_proven {
            return Proof::Unknown(
                "This OS has no recorded guarantee against PID reuse, so the live group cannot be proven to be the agent's"
                    .into(),
            );
        }
        let pids = match members(pgid) {
            Ok(pids) => pids,
            Err(error) => return Proof::Unknown(error),
        };
        if pids.is_empty() {
            return match group_signal(pgid) {
                Group::Gone => Proof::Gone,
                _ => Proof::Unknown("The group is live but none of its members are visible".into()),
            };
        }
        let mut owned = Vec::with_capacity(pids.len());
        for pid in pids {
            let session = unsafe { libc::getsid(pid) };
            if session == sid {
                owned.push(pid);
            } else if session < 0 && errno() == libc::ESRCH {
                continue;
            } else {
                return Proof::Unknown("A group member is outside the recorded session".into());
            }
        }
        if owned.is_empty() {
            return match group_signal(pgid) {
                Group::Gone => Proof::Gone,
                _ => Proof::Unknown("The group is live but none of its members are visible".into()),
            };
        }
        Proof::Owned(owned)
    }

    pub(in super::super) fn probe_group(
        record: Option<&ProcessRecord>,
        policy: ProofPolicy,
    ) -> Probe {
        let Some(record) = record else {
            // A backend that spawned and hung before its `ready` has no
            // record; no record is not proof that its tree is gone.
            return probe(
                ProcessState::Unknown,
                "No process record was journaled, so the tree cannot be proven gone",
            );
        };
        let (Some(pgid), Some(sid)) = (record.pgid, record.service_sid) else {
            return probe(
                ProcessState::Unknown,
                "The process record lacks its group or session identity",
            );
        };
        if let Some(pid) = record.pid {
            if unsafe { libc::kill(pid as i32, 0) } == 0 || errno() == libc::EPERM {
                let now = process_start_time(pid);
                if now.is_none() || now != record.process_start_time {
                    return probe(
                        ProcessState::Unknown,
                        "A process with the leader's PID is running but its start time does not match",
                    );
                }
            }
        }
        match prove(pgid, sid, policy) {
            Proof::Gone => probe(ProcessState::Exited, "The whole process group is gone"),
            Proof::Owned(pids) => Probe {
                state: ProcessState::Alive,
                reason: format!(
                    "{} process{} of the agent's group are still running in the service session",
                    pids.len(),
                    if pids.len() == 1 { "" } else { "es" }
                ),
            },
            Proof::Unknown(reason) => probe(ProcessState::Unknown, &reason),
        }
    }

    /// Signal one proven member through a pidfd, re-checking its group and
    /// session after the pidfd pins it.
    #[cfg(target_os = "linux")]
    fn kill_member(pid: i32, pgid: i32, sid: i32) -> Result<(), String> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
        if fd < 0 {
            return if errno() == libc::ESRCH {
                Ok(())
            } else {
                Err("Cannot pin a group member (pidfd_open failed)".into())
            };
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        match linux_stat(pid) {
            Some(s) if s.pgrp == pgid && s.session == sid => {}
            _ => {
                // Gone, or the PID now names something else: never signal.
                let alive = unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        fd.as_raw_fd(),
                        0,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    )
                } == 0;
                return if alive {
                    Err("A group member left the recorded group or session".into())
                } else {
                    Ok(())
                };
            }
        }
        let sent = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if sent < 0 && errno() != libc::ESRCH {
            return Err("Cannot signal a group member".into());
        }
        Ok(())
    }

    /// End the whole group: re-enumerate, re-prove and signal until
    /// `kill(-pgid, 0)` answers ESRCH, for at most `budget`. Stops without
    /// signalling as soon as a round cannot prove ownership.
    pub(in super::super) async fn terminate_group(
        record: &ProcessRecord,
        policy: ProofPolicy,
        budget: Duration,
    ) -> Termination {
        let done = |confirmed: bool, detail: &str| Termination {
            method: "processGroup",
            confirmed,
            detail: detail.into(),
        };
        let first = probe_group(Some(record), policy);
        match first.state {
            ProcessState::Exited => return done(true, "The process group had already exited"),
            ProcessState::Unknown => {
                return done(false, &format!("Not signalled: {}", first.reason))
            }
            ProcessState::Alive => {}
        }
        let (Some(pgid), Some(sid)) = (record.pgid, record.service_sid) else {
            return done(false, "The process record lacks its group identity");
        };
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let pids = match prove(pgid, sid, policy) {
                Proof::Gone => return done(true, "The whole process group exited"),
                Proof::Unknown(reason) => {
                    return done(false, &format!("Stopped: {reason}"));
                }
                Proof::Owned(pids) => pids,
            };
            #[cfg(target_os = "linux")]
            for pid in pids {
                if let Err(error) = kill_member(pid, pgid, sid) {
                    return done(false, &format!("Stopped: {error}"));
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = pids;
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return done(
                    false,
                    "Group members were still running when the time ran out",
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[cfg(windows)]
pub(super) fn probe_job(job: Option<&super::windows::ProcessJob>) -> Probe {
    let Some(job) = job else {
        return probe(
            ProcessState::Unknown,
            "The service no longer holds the agent's job, so its processes cannot be counted",
        );
    };
    match job.active_processes() {
        Ok(0) => probe(
            ProcessState::Exited,
            "The agent's job has no running processes",
        ),
        Ok(n) => Probe {
            state: ProcessState::Alive,
            reason: format!("The agent's job still has {n} running processes"),
        },
        Err(error) => probe(ProcessState::Unknown, &error),
    }
}

#[cfg(windows)]
pub(super) async fn terminate_job(job: &super::windows::ProcessJob) -> Termination {
    let result = match job.terminate() {
        Ok(()) => job.confirm_empty().await,
        Err(error) => Err(error),
    };
    Termination {
        method: "jobObject",
        confirmed: result.is_ok(),
        detail: result
            .err()
            .unwrap_or_else(|| "The agent's job has no running processes".into()),
    }
}

fn cap(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn compact_target(target: &Value) -> Option<Value> {
    if !target.is_object() {
        return None;
    }
    Some(json!({
        "chromeTabId": target["chromeTabId"],
        "tabRef": target["tabRef"],
        "targetId": target["targetId"],
        "url": target["url"].as_str().map(|u| cap(u, MAX_URL)),
        "title": target["title"].as_str().map(|t| cap(t, MAX_TITLE)),
        "provenance": target["provenance"],
    }))
}

#[derive(Default)]
struct Action {
    sequence: u64,
    tool: String,
    tool_class: String,
    target: Option<Value>,
    reported: bool,
    verdict: Option<String>,
    possibly_executed: Option<bool>,
}

/// Actions, artifacts and unresolved tools of one turn, from its evidence.
/// The journal's targets are already redacted by the event gate.
#[derive(Default)]
struct Turn {
    actions: BTreeMap<String, Action>,
    artifacts: Vec<(String, Value)>,
    unresolved: Vec<Value>,
}
impl Turn {
    fn collect<'a>(events: impl IntoIterator<Item = &'a Event>, message_id: &str) -> Self {
        let mut turn = Self::default();
        let mut artifact_states: BTreeMap<String, String> = BTreeMap::new();
        for event in events {
            let d = &event.data;
            match event.kind.as_str() {
                "artifactReady" | "artifactUnavailable" | "artifactState" => {
                    if let Some(id) = d["artifactId"].as_str() {
                        let state = match event.kind.as_str() {
                            "artifactReady" => "ready".to_string(),
                            "artifactUnavailable" => {
                                format!("unavailable: {}", d["reason"].as_str().unwrap_or(""))
                            }
                            _ => d["state"].as_str().unwrap_or("").to_string(),
                        };
                        artifact_states.insert(id.into(), state);
                    }
                    continue;
                }
                _ => {}
            }
            if d["messageId"].as_str() != Some(message_id) {
                continue;
            }
            let action_id = d["actionId"].as_str().unwrap_or("").to_string();
            match event.kind.as_str() {
                "actionAdmitted" => {
                    let action = turn.actions.entry(action_id).or_default();
                    action.sequence = event.sequence;
                    action.tool = d["tool"].as_str().unwrap_or("").into();
                    action.tool_class = d["toolClass"].as_str().unwrap_or("").into();
                    for output in d["outputs"].as_array().into_iter().flatten() {
                        if let Some(id) = output["artifactId"].as_str() {
                            turn.artifacts.push((
                                id.into(),
                                json!({"artifactId": id, "name": output["name"], "kind": output["kind"]}),
                            ));
                        }
                    }
                }
                "browserActionTarget" => {
                    let action = turn.actions.entry(action_id).or_default();
                    if action.sequence == 0 {
                        action.sequence = event.sequence;
                    }
                    if action.tool.is_empty() {
                        action.tool = d["tool"].as_str().unwrap_or("").into();
                        action.tool_class = d["toolClass"].as_str().unwrap_or("").into();
                    }
                    action.reported = true;
                    let observed = &d["observed"];
                    action.target = compact_target(&observed["after"])
                        .or_else(|| compact_target(&observed["before"]));
                }
                "actionObserved" => {
                    let action = turn.actions.entry(action_id).or_default();
                    action.verdict = d["verdict"].as_str().map(String::from);
                    action.possibly_executed = d["possiblyExecuted"].as_bool();
                }
                "artifactCreated" => {
                    if let Some(id) = d["artifactId"].as_str() {
                        turn.artifacts.push((
                            id.into(),
                            json!({"artifactId": id, "name": d["name"], "kind": d["kind"]}),
                        ));
                    }
                }
                "batchInterrupted" => {
                    turn.unresolved
                        .extend(d["unresolved"].as_array().cloned().unwrap_or_default());
                }
                _ => {}
            }
        }
        for (id, artifact) in turn.artifacts.iter_mut() {
            artifact["state"] = artifact_states
                .get(id)
                .cloned()
                .unwrap_or_else(|| "pending".into())
                .into();
        }
        turn
    }

    fn ordered(&self) -> Vec<(&String, &Action)> {
        let mut actions: Vec<_> = self.actions.iter().collect();
        actions.sort_by_key(|(_, a)| a.sequence);
        actions
    }

    fn verdict(action: &Action) -> &str {
        match (&action.verdict, action.reported) {
            (Some(v), _) => v,
            (None, true) => "reported",
            (None, false) => "notReported",
        }
    }

    /// Everything that acted and is not a failure known not to have run.
    fn possibly_executed(&self) -> Vec<Value> {
        self.ordered()
            .into_iter()
            .filter(|(_, a)| a.tool_class != "observe")
            .filter(|(_, a)| {
                !(a.verdict.as_deref() == Some("failed") && a.possibly_executed == Some(false))
            })
            .map(|(id, a)| {
                json!({
                    "actionId": id,
                    "tool": a.tool,
                    "target": a.target,
                    "verdict": Self::verdict(a),
                })
            })
            .collect()
    }

    fn last_targets(&self) -> Vec<Value> {
        let mut seen = Vec::new();
        let mut targets = Vec::new();
        for (_, action) in self.ordered().into_iter().rev() {
            let Some(target) = &action.target else {
                continue;
            };
            let key = if target["chromeTabId"].is_null() {
                target["targetId"].to_string()
            } else {
                target["chromeTabId"].to_string()
            };
            if !seen.contains(&key) {
                seen.push(key);
                targets.push(target.clone());
            }
            if targets.len() == MAX_LAST_TARGETS {
                break;
            }
        }
        targets.reverse();
        targets
    }
}

/// Keep the last `max` entries; the dropped count goes in `truncated`.
fn bounded(mut items: Vec<Value>, max: usize) -> (Vec<Value>, usize) {
    let dropped = items.len().saturating_sub(max);
    items.drain(..dropped);
    (items, dropped)
}

/// The turn part of `conversationRecoveryReview`: actions with verdicts and
/// `possiblyExecuted`, unresolved tools, artifacts and last targets.
pub(super) fn review_turn<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    message_id: &str,
) -> Value {
    let turn = Turn::collect(events, message_id);
    let actions: Vec<Value> = turn
        .ordered()
        .into_iter()
        .map(|(id, a)| {
            json!({
                "actionId": id,
                "tool": a.tool,
                "toolClass": a.tool_class,
                "reported": a.reported,
                "verdict": Turn::verdict(a),
                "possiblyExecuted": a.possibly_executed,
                "target": a.target,
            })
        })
        .collect();
    let (actions, truncated) = bounded(actions, 64);
    let (unresolved, unresolved_truncated) = bounded(turn.unresolved.clone(), 64);
    let (artifacts, artifacts_truncated) = bounded(
        turn.artifacts.iter().map(|(_, a)| a.clone()).collect(),
        MAX_ARTIFACTS,
    );
    json!({
        "messageId": message_id,
        "actions": actions,
        "truncated": truncated,
        "unresolved": unresolved,
        "unresolvedTruncated": unresolved_truncated,
        "artifacts": artifacts,
        "artifactsTruncated": artifacts_truncated,
        "lastTargets": turn.last_targets(),
    })
}

/// `recoveryContext`, the first event of a fork.
pub(super) fn recovery_context<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    from_conversation: &str,
    from_message: &str,
    from_sequence: u64,
) -> Value {
    let turn = Turn::collect(events, from_message);
    let (possibly_executed, truncated) = bounded(turn.possibly_executed(), MAX_POSSIBLY_EXECUTED);
    let (artifacts, artifacts_truncated) = bounded(
        turn.artifacts.iter().map(|(_, a)| a.clone()).collect(),
        MAX_ARTIFACTS,
    );
    let mut context = json!({
        "v": 1,
        "fromConversationId": from_conversation,
        "fromMessageId": from_message,
        "fromSequence": from_sequence,
        "outcome": "unknown",
        "possiblyExecuted": possibly_executed,
        "lastTargets": turn.last_targets(),
        "artifacts": artifacts,
    });
    if truncated > 0 {
        context["truncated"] = truncated.into();
    }
    if artifacts_truncated > 0 {
        context["artifactsTruncated"] = artifacts_truncated.into();
    }
    context
}

/// `recoveryForked`, journaled in the original conversation.
pub(super) fn recovery_forked(message_id: Option<&str>, forked_to: &str) -> Value {
    json!({"v": 1, "messageId": message_id, "forkedTo": forked_to})
}

fn describe_target(target: &Value) -> String {
    let url = target["url"].as_str().map(|u| cap(u, PREFACE_URL));
    match (target["chromeTabId"].as_i64(), url) {
        (Some(tab), Some(url)) => format!("chrome-tab:{tab} {url}"),
        (Some(tab), None) => format!("chrome-tab:{tab}"),
        (None, Some(url)) => url,
        (None, None) => "an unrecorded page".into(),
    }
}

/// The text prepended to the fork's first message (`afterRecovery`).
pub(super) fn fork_preface(context: &Value) -> String {
    let actions: Vec<String> = context["possiblyExecuted"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_POSSIBLY_EXECUTED)
        .enumerate()
        .map(|(i, a)| {
            let tool = cap(a["tool"].as_str().unwrap_or("a browser tool"), 64);
            let target = if a["target"].is_object() {
                describe_target(&a["target"])
            } else {
                "an unrecorded page".into()
            };
            format!(
                "{}. {tool} on {target} ({})",
                i + 1,
                cap(a["verdict"].as_str().unwrap_or("notReported"), 32)
            )
        })
        .collect();
    let mut list = if actions.is_empty() {
        "none were recorded".to_string()
    } else {
        actions.join("; ")
    };
    if let Some(more) = context["truncated"].as_u64().filter(|n| *n > 0) {
        list.push_str(&format!("; and {more} earlier"));
    }
    let tabs: Vec<String> = context["lastTargets"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_LAST_TARGETS)
        .map(describe_target)
        .collect();
    let tabs = if tabs.is_empty() {
        String::new()
    } else {
        format!(" Last pages: {}.", tabs.join(", "))
    };
    format!(
        "[This conversation continues one whose last request ended with an unknown outcome. These actions may already have taken effect: {list}.{tabs} Observe the pages before acting. Do not redo them unless this message asks you to.]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64, kind: &str, data: Value) -> Event {
        Event {
            sequence,
            at: 0,
            kind: kind.into(),
            data,
        }
    }

    #[test]
    fn process_record_is_the_last_ready_without_a_later_exit() {
        let ready = |pid: u64| json!({"pid": pid, "pgid": pid, "processStartTime": 7, "serviceSid": 50, "generation": 3});
        let events = vec![
            event(1, "ready", ready(100)),
            event(2, "exited", json!({"processExitConfirmed": true})),
            event(3, "ready", ready(200)),
            event(4, "processUnconfirmed", json!({})),
        ];
        let record = ProcessRecord::from_events(&events).unwrap();
        assert_eq!(
            record,
            ProcessRecord {
                generation: Some(3),
                pid: Some(200),
                pgid: Some(200),
                process_start_time: Some(7),
                service_sid: Some(50)
            }
        );
        assert!(ProcessRecord::from_events(&events[..2]).is_none());
        // A ready from before this release has no group identity.
        let old =
            ProcessRecord::from_events(&[event(1, "ready", json!({"pid": 9, "sessionId": "s"}))])
                .unwrap();
        assert_eq!((old.pgid, old.service_sid), (None, None));
    }

    fn turn_events() -> Vec<Event> {
        let mut events = vec![event(1, "turnStarted", json!({"messageId": "m"}))];
        for i in 0..25u64 {
            let id = format!("a{i:02}");
            events.push(event(10 + i * 3, "actionAdmitted", json!({"messageId": "m", "actionId": id, "tool": "chrome_use_click", "toolClass": "act", "outputs": []})));
            events.push(event(11 + i * 3, "browserActionTarget", json!({"messageId": "m", "actionId": id, "tool": "chrome_use_click", "observed": {"after": {"chromeTabId": 100 + (i % 3), "targetId": "t", "url": format!("https://shop.example/p/{i}"), "provenance": "actionReply"}}})));
            events.push(event(12 + i * 3, "actionObserved", json!({"messageId": "m", "actionId": id, "verdict": "uncertain", "possiblyExecuted": true})));
        }
        events.push(event(200, "actionAdmitted", json!({"messageId": "m", "actionId": "read", "tool": "chrome_use_read", "toolClass": "observe", "outputs": []})));
        events.push(event(201, "actionAdmitted", json!({"messageId": "m", "actionId": "nope", "tool": "chrome_use_fill", "toolClass": "act", "outputs": []})));
        events.push(event(202, "actionObserved", json!({"messageId": "m", "actionId": "nope", "verdict": "failed", "possiblyExecuted": false})));
        events.push(event(203, "actionAdmitted", json!({"messageId": "m", "actionId": "dl", "tool": "chrome_use_download", "toolClass": "capture", "outputs": [{"artifactId": "f1", "name": "r.pdf", "kind": "download"}]})));
        events.push(event(
            204,
            "artifactUnavailable",
            json!({"artifactId": "f1", "reason": "interrupted"}),
        ));
        events.push(event(205, "actionAdmitted", json!({"messageId": "other", "actionId": "x", "tool": "chrome_use_click", "toolClass": "act", "outputs": []})));
        events.push(event(206, "batchInterrupted", json!({"messageId": "m", "unresolved": [{"toolCallId": "c1", "name": "chrome_use_click", "actionId": "dl", "executed": "admitted"}]})));
        events
    }

    #[test]
    fn fork_context_and_preface_are_bounded_and_list_only_possible_effects() {
        let events = turn_events();
        let context = recovery_context(&events, "conv", "m", 206);
        let listed = context["possiblyExecuted"].as_array().unwrap();
        assert_eq!(listed.len(), MAX_POSSIBLY_EXECUTED);
        // 25 clicks plus the unreported download; reads and known failures are not listed.
        assert_eq!(context["truncated"], 6);
        assert_eq!(listed.last().unwrap()["actionId"], "dl");
        assert_eq!(listed.last().unwrap()["verdict"], "notReported");
        assert!(!listed
            .iter()
            .any(|a| a["actionId"] == "read" || a["actionId"] == "nope" || a["actionId"] == "x"));
        assert_eq!(context["lastTargets"].as_array().unwrap().len(), 3);
        assert_eq!(context["artifacts"][0]["state"], "unavailable: interrupted");
        let preface = fork_preface(&context);
        assert!(preface.starts_with(
            "[This conversation continues one whose last request ended with an unknown outcome."
        ));
        assert!(preface.contains("chrome_use_click on chrome-tab:"));
        assert!(preface.contains("and 6 earlier"));
        assert!(preface.ends_with("Do not redo them unless this message asks you to.]"));
        assert!(preface.len() < 8 * 1024);
        assert!(serde_json::to_vec(&context).unwrap().len() < 64 * 1024);
        let review = review_turn(&events, "m");
        assert_eq!(review["unresolved"][0]["executed"], "admitted");
        assert_eq!(review["actions"].as_array().unwrap().len(), 28);
        let empty = fork_preface(&recovery_context(&[], "conv", "m", 0));
        assert!(empty.contains("may already have taken effect: none were recorded."));
    }

    #[test]
    fn long_urls_are_capped_in_context_and_preface() {
        let url = format!("https://a.example/{}", "x".repeat(5000));
        let events = vec![
            event(
                1,
                "actionAdmitted",
                json!({"messageId": "m", "actionId": "a", "tool": "chrome_use_open", "toolClass": "act", "outputs": []}),
            ),
            event(
                2,
                "browserActionTarget",
                json!({"messageId": "m", "actionId": "a", "observed": {"after": {"chromeTabId": 1, "url": url, "title": "t".repeat(900)}}}),
            ),
        ];
        let context = recovery_context(&events, "c", "m", 2);
        let target = &context["possiblyExecuted"][0]["target"];
        assert_eq!(target["url"].as_str().unwrap().chars().count(), MAX_URL);
        assert_eq!(target["title"].as_str().unwrap().chars().count(), MAX_TITLE);
        assert!(fork_preface(&context).len() < 1024);
    }

    #[cfg(unix)]
    mod unix_probe {
        use super::super::*;
        use std::process::{Command, Stdio};
        use std::time::Duration;

        const PROVEN: ProofPolicy = ProofPolicy {
            pid_allocation_proven: true,
        };

        fn errno() -> i32 {
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        }

        /// Run `script` as the leader of a fresh session (and so a fresh
        /// group), the way the service runs its backends under its own
        /// `setsid`, and reap it. The record's session is that new session,
        /// whatever session the test runner itself is in.
        fn session_group(script: &str) -> ProcessRecord {
            use std::os::unix::process::CommandExt;
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", script])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .stdin(Stdio::null());
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut leader = command.spawn().unwrap();
            let pid = leader.id();
            let start = process_start_time(pid);
            leader.wait().unwrap();
            ProcessRecord {
                generation: Some(1),
                pid: Some(pid),
                pgid: Some(pid as i32),
                process_start_time: start,
                service_sid: Some(pid as i32),
            }
        }

        /// The usual unconfirmed case: leader reaped, children keep the group.
        fn orphaned_group() -> ProcessRecord {
            let record = session_group("sleep 60 & sleep 60 & exit 0");
            let pgid = record.pgid.unwrap();
            assert_ne!(Some(pgid), current_session(), "not the runner's session");
            assert_eq!(
                unsafe { libc::kill(-pgid, 0) },
                0,
                "children keep the group alive"
            );
            record
        }

        fn kill_group(pgid: i32) {
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }

        #[tokio::test]
        async fn leader_dead_group_alive_is_alive_and_terminate_waits_for_esrch() {
            let record = orphaned_group();
            let pgid = record.pgid.unwrap();
            assert_eq!(
                unsafe { libc::kill(-pgid, 0) },
                0,
                "children keep the group alive"
            );
            let probe = probe_group(Some(&record), PROVEN);
            assert_eq!(probe.state, ProcessState::Alive, "{}", probe.reason);
            if cfg!(any(target_os = "linux", target_os = "macos")) {
                let running = probe_group(Some(&record), ProofPolicy::running_os());
                assert_eq!(running.state, ProcessState::Alive, "{}", running.reason);
            }
            let ended = terminate_group(&record, PROVEN, Duration::from_secs(5)).await;
            assert!(ended.confirmed, "{}", ended.detail);
            assert!(unsafe { libc::kill(-pgid, 0) } < 0 && errno() == libc::ESRCH);
            assert_eq!(
                probe_group(Some(&record), PROVEN).state,
                ProcessState::Exited
            );
            assert_eq!(ended.event(Some("m"))["method"], "processGroup");
        }

        #[tokio::test]
        async fn without_the_os_proof_a_live_group_is_unknown_and_never_signalled() {
            let record = orphaned_group();
            let pgid = record.pgid.unwrap();
            let policy = ProofPolicy {
                pid_allocation_proven: false,
            };
            assert_eq!(
                probe_group(Some(&record), policy).state,
                ProcessState::Unknown
            );
            let ended = terminate_group(&record, policy, Duration::from_secs(1)).await;
            assert!(!ended.confirmed);
            assert_eq!(unsafe { libc::kill(-pgid, 0) }, 0, "nothing was signalled");
            kill_group(pgid);
        }

        #[tokio::test]
        async fn members_in_another_session_make_the_group_unknown() {
            let record = orphaned_group();
            let pgid = record.pgid.unwrap();
            let foreign = ProcessRecord {
                service_sid: Some(record.service_sid.unwrap() + 1),
                ..record.clone()
            };
            assert_eq!(
                probe_group(Some(&foreign), PROVEN).state,
                ProcessState::Unknown
            );
            let ended = terminate_group(&foreign, PROVEN, Duration::from_secs(1)).await;
            assert!(!ended.confirmed);
            assert_eq!(unsafe { libc::kill(-pgid, 0) }, 0, "nothing was signalled");
            kill_group(pgid);
        }

        #[test]
        fn a_gone_group_is_exited_and_a_reused_leader_pid_is_unknown() {
            let record = orphaned_group();
            let pgid = record.pgid.unwrap();
            kill_group(pgid);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while unsafe { libc::kill(-pgid, 0) } == 0 {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                probe_group(Some(&record), PROVEN).state,
                ProcessState::Exited
            );
            // The recorded leader PID is this live test process with another start time.
            let me = ProcessRecord {
                pid: Some(std::process::id()),
                process_start_time: Some(1),
                ..record.clone()
            };
            assert_eq!(probe_group(Some(&me), PROVEN).state, ProcessState::Unknown);
            let missing = ProcessRecord {
                pgid: None,
                ..record
            };
            assert_eq!(
                probe_group(Some(&missing), PROVEN).state,
                ProcessState::Unknown
            );
            assert_eq!(probe_group(None, PROVEN).state, ProcessState::Unknown);
            let p = probe_group(None, PROVEN).review(None);
            assert_eq!(
                (p["canFork"].as_bool(), p["canTerminate"].as_bool()),
                (Some(false), Some(false))
            );
        }

        #[tokio::test]
        async fn a_child_forked_during_terminate_is_caught_by_a_later_round() {
            // Members keep forking replacements; terminate must loop until ESRCH.
            let record = session_group(
                "for i in 1 2 3 4; do (trap '' TERM; while :; do sleep 0.01 & wait; done) & done; exit 0",
            );
            let pid = record.pid.unwrap();
            let ended = terminate_group(&record, PROVEN, Duration::from_secs(5)).await;
            assert!(ended.confirmed, "{}", ended.detail);
            assert!(unsafe { libc::kill(-(pid as i32), 0) } < 0);
        }

        /// A `ready` recorded in session 1 (PID 1's session in a container) or
        /// with no session at all proves nothing: the live group is `unknown`
        /// and is never signalled.
        #[tokio::test]
        async fn session_one_or_a_missing_session_is_unknown() {
            let record = orphaned_group();
            let pgid = record.pgid.unwrap();
            for service_sid in [Value::from(1), Value::from(0), Value::Null] {
                let ready = Event {
                    sequence: 1,
                    at: 0,
                    kind: "ready".into(),
                    data: json!({
                        "pid": record.pid,
                        "pgid": pgid,
                        "processStartTime": record.process_start_time,
                        "serviceSid": service_sid,
                    }),
                };
                let loaded = ProcessRecord::from_events([&ready]).unwrap();
                assert_eq!(loaded.service_sid, None);
                let probe = probe_group(Some(&loaded), PROVEN);
                assert_eq!(probe.state, ProcessState::Unknown, "{}", probe.reason);
                let ended = terminate_group(&loaded, PROVEN, Duration::from_secs(1)).await;
                assert!(!ended.confirmed);
                assert_eq!(unsafe { libc::kill(-pgid, 0) }, 0, "nothing was signalled");
            }
            kill_group(pgid);
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn linux_stat_fields_follow_the_last_paren() {
            let stat = parse_linux_stat(
                "42 (a) b) (c) S 1 7 9 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 12345 0",
            )
            .unwrap();
            assert_eq!((stat.pgrp, stat.session, stat.start_time), (7, 9, 12345));
        }
    }

    #[cfg(windows)]
    mod windows_job {
        use super::super::*;
        use crate::agent_panel::windows::ProcessJob;
        use std::time::Duration;

        /// 14.1: the held job's probe counts its processes, terminate empties
        /// it, and without the handle (a restarted service) it is unknown.
        #[tokio::test]
        async fn held_job_is_counted_terminated_and_unknown_after_restart() {
            let mut command = tokio::process::Command::new("python");
            command
                .args(["-c", "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)']); time.sleep(120)"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            let (mut child, job) = ProcessJob::spawn(&mut command).await.unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while job.active_processes().unwrap() < 2 {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "descendant never started"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let probe = probe_job(Some(&job));
            assert_eq!(probe.state, ProcessState::Alive, "{}", probe.reason);
            let ended = terminate_job(&job).await;
            assert!(ended.confirmed, "{}", ended.detail);
            assert_eq!(ended.method, "jobObject");
            child.wait().await.unwrap();
            assert_eq!(probe_job(Some(&job)).state, ProcessState::Exited);
            drop(job);
            let restarted = probe_job(None);
            assert_eq!(restarted.state, ProcessState::Unknown);
            let review = restarted.review(None);
            assert_eq!(
                (
                    review["canFork"].as_bool(),
                    review["canTerminate"].as_bool()
                ),
                (Some(false), Some(false))
            );
        }
    }
}
