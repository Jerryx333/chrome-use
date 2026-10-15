//! Advisory progress signals for a running turn, and the turn summary.
//! Like `native/progress.rs`, nothing here stops, retries or changes success:
//! it only says when a turn looks stuck and when it moved again. Every count
//! comes from journaled evidence, never from model text.
//!
//! This module computes event payloads; the engine journals them.
#![allow(dead_code)] // Wired into the service by the integration step.

use super::conversation::Event;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub(super) const IDENTICAL_LIMIT: usize = 4;
pub(super) const FAILURE_LIMIT: usize = 3;
pub(super) const QUIET: Duration = Duration::from_secs(180);
/// An open tool suppresses `noActivity` until its own timeout plus this.
pub(super) const OPEN_TOOL_GRACE: Duration = Duration::from_secs(30);
/// Assumed when an open tool call did not say how long it may take.
pub(super) const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ACTION_IDS: usize = 16;
const EXCLUDED_TOOLS: &[&str] = &["chrome_use_wait"];
const IGNORED_ARGUMENTS: &[&str] = &["timeoutMs", "session"];

/// One reported chrome_use action, as the stall detector sees it.
#[derive(Clone, Debug, Default)]
pub(super) struct ActionSample {
    pub sequence: u64,
    pub action_id: String,
    pub tool: String,
    pub arguments: Value,
    pub after_target_id: Option<String>,
    /// Already redacted by the event gate.
    pub after_url: Option<String>,
    pub result_digest: Option<String>,
    pub failed: bool,
    pub error_code: Option<String>,
    pub native_no_progress: bool,
}

/// Arguments in a fixed key order, so equal calls hash equally whatever
/// order the agent wrote them in.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn hash(parts: &Value) -> String {
    hex::encode(Sha256::digest(canonical(parts).to_string().as_bytes()))
}

impl ActionSample {
    fn fingerprint(&self) -> String {
        let mut arguments = self.arguments.clone();
        if let Some(map) = arguments.as_object_mut() {
            for key in IGNORED_ARGUMENTS {
                map.remove(*key);
            }
        }
        hash(&json!([
            self.tool,
            arguments,
            self.after_target_id,
            self.after_url,
            self.result_digest
        ]))
    }
    fn failure_key(&self) -> Option<String> {
        self.failed
            .then(|| hash(&json!([self.tool, self.error_code])))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StallKind {
    RepeatedIdenticalResult,
    RepeatedFailure,
    NativeNoProgress,
    NoActivity,
}
impl StallKind {
    fn name(self) -> &'static str {
        match self {
            Self::RepeatedIdenticalResult => "repeatedIdenticalResult",
            Self::RepeatedFailure => "repeatedFailure",
            Self::NativeNoProgress => "nativeNoProgress",
            Self::NoActivity => "noActivity",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Signal {
    Stalled {
        kind: StallKind,
        count: Option<usize>,
        quiet_ms: Option<u64>,
        action_ids: Vec<String>,
        source_sequences: [u64; 2],
    },
    Resumed {
        source_sequence: u64,
    },
}
impl Signal {
    /// The event kind and payload to journal.
    pub fn event(&self, message_id: &str) -> (&'static str, Value) {
        match self {
            Signal::Stalled {
                kind,
                count,
                quiet_ms,
                action_ids,
                source_sequences,
            } => (
                "progressStalled",
                json!({
                    "v": 1,
                    "messageId": message_id,
                    "kind": kind.name(),
                    "count": count,
                    "quietMs": quiet_ms,
                    "actionIds": action_ids,
                    "sourceSequences": source_sequences,
                }),
            ),
            Signal::Resumed { source_sequence } => (
                "progressResumed",
                json!({"v": 1, "messageId": message_id, "sourceSequence": source_sequence}),
            ),
        }
    }
}

#[derive(Default)]
struct Run {
    key: Option<String>,
    action_ids: Vec<String>,
    first: u64,
    last: u64,
    count: usize,
}
impl Run {
    fn push(&mut self, key: Option<String>, sample: &ActionSample) {
        if key.is_none() || key != self.key {
            *self = Run {
                key: key.clone(),
                first: sample.sequence,
                ..Run::default()
            };
        }
        if key.is_none() {
            return;
        }
        self.count += 1;
        self.last = sample.sequence;
        self.action_ids.push(sample.action_id.clone());
        if self.action_ids.len() > MAX_ACTION_IDS {
            self.action_ids.remove(0);
        }
    }
    fn stalled(&self, kind: StallKind) -> Signal {
        Signal::Stalled {
            kind,
            count: Some(self.count),
            quiet_ms: None,
            action_ids: self.action_ids.clone(),
            source_sequences: [self.first, self.last],
        }
    }
}

/// An action episode stays open while each new action continues it.
struct Episode {
    kind: StallKind,
    key: String,
}

/// An open tool call, for `noActivity` suppression.
#[derive(Clone, Copy, Debug)]
pub(super) struct OpenTool {
    pub started: Instant,
    pub timeout: Option<Duration>,
}

/// Per-turn detector. One `progressStalled` per episode; the action that
/// breaks the pattern, or any backend event for `noActivity`, resumes it.
pub(super) struct StallDetector {
    identical: Run,
    failures: Run,
    action_episode: Option<Episode>,
    quiet_episode: bool,
    last_activity: Instant,
    last_sequence: u64,
}

impl StallDetector {
    pub fn new(turn_started_sequence: u64, now: Instant) -> Self {
        Self {
            identical: Run::default(),
            failures: Run::default(),
            action_episode: None,
            quiet_episode: false,
            last_activity: now,
            last_sequence: turn_started_sequence,
        }
    }

    /// A reported chrome_use action of this turn. Its report is activity too.
    pub fn observe_action(&mut self, sample: &ActionSample, now: Instant) -> Vec<Signal> {
        let mut signals = self.activity(sample.sequence, now);
        if EXCLUDED_TOOLS.contains(&sample.tool.as_str()) {
            return signals;
        }
        let fingerprint = sample.fingerprint();
        let failure_key = sample.failure_key();
        if let Some(episode) = &self.action_episode {
            let continues = match episode.kind {
                StallKind::RepeatedFailure => failure_key.as_deref() == Some(&episode.key),
                StallKind::NativeNoProgress => {
                    sample.native_no_progress && fingerprint == episode.key
                }
                _ => fingerprint == episode.key,
            };
            if !continues {
                self.action_episode = None;
                signals.push(Signal::Resumed {
                    source_sequence: sample.sequence,
                });
            }
        }
        self.identical.push(Some(fingerprint.clone()), sample);
        self.failures.push(failure_key.clone(), sample);
        if self.action_episode.is_some() {
            return signals;
        }
        let opened = if sample.native_no_progress {
            Some((
                StallKind::NativeNoProgress,
                fingerprint.clone(),
                Signal::Stalled {
                    kind: StallKind::NativeNoProgress,
                    count: None,
                    quiet_ms: None,
                    action_ids: vec![sample.action_id.clone()],
                    source_sequences: [sample.sequence, sample.sequence],
                },
            ))
        } else if self.failures.count >= FAILURE_LIMIT {
            Some((
                StallKind::RepeatedFailure,
                failure_key.unwrap_or_default(),
                self.failures.stalled(StallKind::RepeatedFailure),
            ))
        } else if self.identical.count >= IDENTICAL_LIMIT {
            Some((
                StallKind::RepeatedIdenticalResult,
                fingerprint,
                self.identical.stalled(StallKind::RepeatedIdenticalResult),
            ))
        } else {
            None
        };
        if let Some((kind, key, signal)) = opened {
            self.action_episode = Some(Episode { kind, key });
            signals.push(signal);
        }
        signals
    }

    /// Any journaled backend event of this turn.
    pub fn backend_activity(&mut self, sequence: u64, now: Instant) -> Vec<Signal> {
        self.activity(sequence, now)
    }

    fn activity(&mut self, sequence: u64, now: Instant) -> Vec<Signal> {
        self.last_activity = now;
        self.last_sequence = self.last_sequence.max(sequence);
        if std::mem::take(&mut self.quiet_episode) {
            vec![Signal::Resumed {
                source_sequence: sequence,
            }]
        } else {
            Vec::new()
        }
    }

    /// From the service's 1 s scheduler. `running` is the `Running` phase;
    /// a pending approval or an open tool still inside its own timeout plus
    /// the grace period means the quiet is expected.
    pub fn tick(
        &mut self,
        now: Instant,
        running: bool,
        pending_request: bool,
        open_tools: &[OpenTool],
    ) -> Option<Signal> {
        if self.quiet_episode || !running || pending_request {
            return None;
        }
        let quiet = now.saturating_duration_since(self.last_activity);
        if quiet < QUIET {
            return None;
        }
        let busy = open_tools.iter().any(|tool| {
            now < tool.started + tool.timeout.unwrap_or(DEFAULT_TOOL_TIMEOUT) + OPEN_TOOL_GRACE
        });
        if busy {
            return None;
        }
        self.quiet_episode = true;
        Some(Signal::Stalled {
            kind: StallKind::NoActivity,
            count: None,
            quiet_ms: Some(quiet.as_millis() as u64),
            action_ids: Vec::new(),
            source_sequences: [self.last_sequence, self.last_sequence],
        })
    }
}

fn step_failed(data: &Value) -> bool {
    let item = &data["item"];
    data["failed"] == true
        || item["is_error"] == true
        || item["isError"] == true
        || item["error"].is_object()
        || matches!(item["status"].as_str(), Some("failed" | "error"))
        || item["result"]["isError"] == true
}

/// `turnSummary` for one message, counted only from journaled events. Call
/// it after `turnEnded` (and the final assessment) are journaled.
pub(super) fn turn_summary<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    message_id: &str,
) -> Value {
    let mut first = None;
    let mut last = 0;
    let mut steps = 0;
    let mut failed_steps = 0;
    let mut admitted = BTreeSet::new();
    let mut reported = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut verdicts: BTreeMap<String, String> = BTreeMap::new();
    let mut artifacts = BTreeSet::new();
    let mut ready = BTreeSet::new();
    let mut unavailable = BTreeSet::new();
    let mut assessment = Value::Null;
    let mut stalls = 0;
    for event in events {
        let d = &event.data;
        match event.kind.as_str() {
            "artifactReady" => {
                if let Some(id) = d["artifactId"].as_str() {
                    ready.insert(id.to_string());
                }
                continue;
            }
            "artifactUnavailable" => {
                if let Some(id) = d["artifactId"].as_str() {
                    unavailable.insert(id.to_string());
                }
                continue;
            }
            _ => {}
        }
        if d["messageId"].as_str() != Some(message_id) {
            continue;
        }
        if event.kind == "turnStarted" || first.is_none() {
            first = Some(first.map_or(event.sequence, |f: u64| f.min(event.sequence)));
        }
        last = last.max(event.sequence);
        let action_id = d["actionId"].as_str().map(String::from);
        match event.kind.as_str() {
            "tool" if d["completed"] == true => {
                steps += 1;
                if step_failed(d) {
                    failed_steps += 1;
                }
                for id in d["actionIds"].as_array().into_iter().flatten() {
                    if let Some(id) = id.as_str() {
                        linked.insert(id.to_string());
                    }
                }
            }
            "actionAdmitted" => {
                admitted.extend(action_id);
                for output in d["outputs"].as_array().into_iter().flatten() {
                    if let Some(id) = output["artifactId"].as_str() {
                        artifacts.insert(id.to_string());
                    }
                }
            }
            "browserActionTarget" => {
                reported.extend(action_id);
            }
            "actionObserved" => {
                if let (Some(id), Some(verdict)) = (action_id, d["verdict"].as_str()) {
                    verdicts.insert(id, verdict.into());
                }
            }
            "artifactCreated" => {
                if let Some(id) = d["artifactId"].as_str() {
                    artifacts.insert(id.to_string());
                }
            }
            "taskAssessment" if d["phase"] == "final" => {
                assessment = d["status"].clone();
            }
            "progressStalled" => stalls += 1,
            _ => {}
        }
    }
    let count = |v: &str| verdicts.values().filter(|x| x.as_str() == v).count();
    json!({
        "v": 1,
        "messageId": message_id,
        "sourceSequences": [first.unwrap_or(0), last],
        "steps": steps,
        "failedSteps": failed_steps,
        "actions": {
            "confirmed": count("confirmed"),
            "failed": count("failed"),
            "uncertain": count("uncertain"),
            "unlinked": reported.difference(&linked).count(),
            "admittedNotReported": admitted.difference(&reported).count(),
        },
        "artifacts": {
            "ready": artifacts.intersection(&ready).count(),
            "unavailable": artifacts.intersection(&unavailable).count(),
        },
        "assessment": assessment,
        "stalls": stalls,
    })
}

/// `contextCompacted`: the backend's signal and the sequence range since the
/// previous compaction or the turn's start.
pub(super) fn context_compacted<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    message_id: &str,
    backend: &str,
    trigger: Option<&str>,
    sequence: u64,
) -> Value {
    let mut start = None;
    for event in events {
        if event.sequence >= sequence || event.data["messageId"].as_str() != Some(message_id) {
            continue;
        }
        match event.kind.as_str() {
            "turnStarted" => start = Some(event.sequence),
            "contextCompacted" => start = Some(event.sequence + 1),
            _ if start.is_none() => start = Some(event.sequence),
            _ => {}
        }
    }
    json!({
        "v": 1,
        "messageId": message_id,
        "backend": backend,
        "trigger": trigger,
        "sourceSequences": [start.unwrap_or(sequence), sequence],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(sequence: u64, digest: &str) -> ActionSample {
        ActionSample {
            sequence,
            action_id: format!("a{sequence}"),
            tool: "chrome_use_read".into(),
            arguments: json!({"selector": "main", "timeoutMs": sequence}),
            after_target_id: Some("T1".into()),
            after_url: Some("https://shop.example/list".into()),
            result_digest: Some(digest.into()),
            ..ActionSample::default()
        }
    }
    fn kinds(signals: &[Signal]) -> Vec<&'static str> {
        signals.iter().map(|s| s.event("m").0).collect()
    }

    #[test]
    fn four_identical_reads_stall_once_and_a_different_result_resumes() {
        let t = Instant::now();
        let mut d = StallDetector::new(1, t);
        let mut all = Vec::new();
        for i in 0..6 {
            all.extend(d.observe_action(&read(10 + i, "same"), t));
        }
        assert_eq!(kinds(&all), vec!["progressStalled"]);
        let (_, event) = all[0].event("m");
        assert_eq!(event["kind"], "repeatedIdenticalResult");
        assert_eq!(event["count"], 4);
        assert_eq!(event["sourceSequences"], json!([10, 13]));
        assert_eq!(event["actionIds"], json!(["a10", "a11", "a12", "a13"]));
        let resumed = d.observe_action(&read(20, "other"), t);
        assert_eq!(resumed[0].event("m").1["sourceSequence"], 20);
        // Key order of the arguments does not make two calls different.
        let mut d = StallDetector::new(1, t);
        let mut reordered = read(1, "x");
        reordered.arguments = json!({"timeoutMs": 9, "selector": "main"});
        for sample in [read(1, "x"), reordered, read(2, "x"), read(3, "x")] {
            d.observe_action(&sample, t);
        }
        assert_eq!(d.identical.count, 4);
    }

    #[test]
    fn paginated_reads_and_waits_never_stall() {
        let t = Instant::now();
        let mut d = StallDetector::new(1, t);
        let mut all = Vec::new();
        for i in 0..6 {
            all.extend(d.observe_action(&read(10 + i, &format!("page {i}")), t));
        }
        for i in 0..6 {
            let wait = ActionSample {
                tool: "chrome_use_wait".into(),
                ..read(20 + i, "same")
            };
            all.extend(d.observe_action(&wait, t));
        }
        assert!(all.is_empty());
    }

    #[test]
    fn three_failures_with_one_code_stall_and_a_success_resumes() {
        let t = Instant::now();
        let mut d = StallDetector::new(1, t);
        let fail = |sequence: u64, code: &str| ActionSample {
            tool: "chrome_use_click".into(),
            failed: true,
            error_code: Some(code.into()),
            result_digest: Some(format!("{sequence}")),
            ..read(sequence, "")
        };
        assert!(d.observe_action(&fail(1, "not_found"), t).is_empty());
        assert!(d.observe_action(&fail(2, "timeout"), t).is_empty());
        assert!(d.observe_action(&fail(3, "not_found"), t).is_empty());
        assert!(d.observe_action(&fail(4, "not_found"), t).is_empty());
        let stalled = d.observe_action(&fail(5, "not_found"), t);
        assert_eq!(stalled[0].event("m").1["kind"], "repeatedFailure");
        assert!(d.observe_action(&fail(6, "not_found"), t).is_empty());
        assert_eq!(
            kinds(&d.observe_action(&read(7, "ok"), t)),
            vec!["progressResumed"]
        );
        let mut native = read(8, "n");
        native.native_no_progress = true;
        assert_eq!(
            d.observe_action(&native, t)[0].event("m").1["kind"],
            "nativeNoProgress"
        );
    }

    #[test]
    fn no_activity_is_suppressed_by_open_tools_and_pending_approvals() {
        let t = Instant::now();
        let mut d = StallDetector::new(5, t);
        let later = t + QUIET + Duration::from_secs(1);
        assert!(d
            .tick(t + Duration::from_secs(10), true, false, &[])
            .is_none());
        assert!(d.tick(later, true, true, &[]).is_none());
        assert!(d.tick(later, false, false, &[]).is_none());
        let long = OpenTool {
            started: t,
            timeout: Some(Duration::from_secs(600)),
        };
        assert!(d.tick(later, true, false, &[long]).is_none());
        let expired = OpenTool {
            started: t,
            timeout: Some(Duration::from_secs(5)),
        };
        let stalled = d.tick(later, true, false, &[expired]).unwrap();
        let (kind, event) = stalled.event("m");
        assert_eq!(
            (kind, event["kind"].as_str()),
            ("progressStalled", Some("noActivity"))
        );
        assert_eq!(
            event["quietMs"],
            (QUIET + Duration::from_secs(1)).as_millis() as u64
        );
        assert!(d
            .tick(later + Duration::from_secs(60), true, false, &[])
            .is_none());
        let resumed = d.backend_activity(9, later + Duration::from_secs(61));
        assert_eq!(resumed, vec![Signal::Resumed { source_sequence: 9 }]);
        assert!(d.backend_activity(10, later).is_empty());
    }

    fn event(sequence: u64, kind: &str, data: Value) -> Event {
        Event {
            sequence,
            at: 0,
            kind: kind.into(),
            data,
        }
    }

    #[test]
    fn summary_counts_only_journaled_evidence_of_its_turn() {
        let events = vec![
            event(1, "user", json!({"messageId": "m"})),
            event(2, "turnStarted", json!({"messageId": "m"})),
            event(
                3,
                "actionAdmitted",
                json!({"messageId": "m", "actionId": "a", "outputs": []}),
            ),
            event(
                4,
                "browserActionTarget",
                json!({"messageId": "m", "actionId": "a"}),
            ),
            event(
                5,
                "actionObserved",
                json!({"messageId": "m", "actionId": "a", "verdict": "confirmed"}),
            ),
            event(
                6,
                "tool",
                json!({"messageId": "m", "completed": true, "actionIds": ["a"], "item": {}}),
            ),
            event(
                7,
                "actionAdmitted",
                json!({"messageId": "m", "actionId": "b", "outputs": [{"artifactId": "f"}]}),
            ),
            event(
                8,
                "browserActionTarget",
                json!({"messageId": "m", "actionId": "b"}),
            ),
            event(9, "artifactReady", json!({"artifactId": "f"})),
            event(
                10,
                "actionObserved",
                json!({"messageId": "m", "actionId": "b", "verdict": "confirmed"}),
            ),
            event(
                11,
                "actionAdmitted",
                json!({"messageId": "m", "actionId": "c", "outputs": []}),
            ),
            event(
                12,
                "tool",
                json!({"messageId": "m", "completed": true, "item": {"is_error": true}}),
            ),
            event(
                13,
                "tool",
                json!({"messageId": "m", "completed": false, "item": {}}),
            ),
            event(14, "progressStalled", json!({"messageId": "m"})),
            event(
                15,
                "actionAdmitted",
                json!({"messageId": "other", "actionId": "z", "outputs": []}),
            ),
            event(16, "turnEnded", json!({"messageId": "m"})),
            event(
                17,
                "taskAssessment",
                json!({"messageId": "m", "phase": "provisional", "status": "checksPassed"}),
            ),
            event(
                18,
                "taskAssessment",
                json!({"messageId": "m", "phase": "final", "status": "someChecksPassed"}),
            ),
        ];
        let summary = turn_summary(&events, "m");
        assert_eq!(summary["sourceSequences"], json!([1, 18]));
        assert_eq!(
            (summary["steps"].as_u64(), summary["failedSteps"].as_u64()),
            (Some(2), Some(1))
        );
        assert_eq!(
            summary["actions"],
            json!({"confirmed": 2, "failed": 0, "uncertain": 0, "unlinked": 1, "admittedNotReported": 1})
        );
        assert_eq!(summary["artifacts"], json!({"ready": 1, "unavailable": 0}));
        assert_eq!(summary["assessment"], "someChecksPassed");
        assert_eq!(summary["stalls"], 1);
        assert_eq!(turn_summary(&events, "nobody")["assessment"], Value::Null);
        let compacted = vec![
            event(2, "turnStarted", json!({"messageId": "m"})),
            event(7, "contextCompacted", json!({"messageId": "m"})),
        ];
        assert_eq!(
            context_compacted(&compacted[..1], "m", "codex", None, 6)["sourceSequences"],
            json!([2, 6])
        );
        assert_eq!(
            context_compacted(&compacted, "m", "claude", Some("auto"), 12)["sourceSequences"],
            json!([8, 12])
        );
    }
}
