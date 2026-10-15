//! Action leases and task assessment (plan 3.3, 4.3).
//!
//! A lease binds one browser action to the turn that admitted it. Its state
//! machine is the one in the plan: `Open` → `Reporting` → `Reported`, or
//! `Open` → `Unreported` at the turn's end → `LateReported` on a late report.
//! Every state is rebuilt from the journal, so a retry after a crash gets the
//! answer that was recorded. This module holds the state and the pure
//! computations; the engine journals what they return.

use super::artifacts::{Artifacts, Output, State};
use super::conversation::Event;
use super::evidence_proto::{CheckKind, ToolClass, Verdict};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub(super) const MAX_OPEN_LEASES: usize = 256;
const MAX_REQUIREMENTS: usize = 12;
const MAX_TEXT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LeaseState {
    Open,
    Reporting,
    Reported,
    Unreported,
    LateReported,
}

/// What one acted-on page looked like after the action.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Landing {
    pub target_id: Option<String>,
    pub url: Option<String>,
    pub from_reply: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Lease {
    pub message_id: String,
    pub generation: u64,
    pub tool: String,
    pub tool_class: ToolClass,
    pub outputs: Vec<Output>,
    pub state: LeaseState,
    /// Admitted before a stop that was requested later.
    pub after_stop: bool,
    /// Its turn has a `turnEnded`.
    pub turn_ended: bool,
    /// Sequence of its `browserActionTarget`.
    pub report_sequence: Option<u64>,
    pub landing: Landing,
    pub verdict: Option<Verdict>,
    /// The recorded check: kind and predicate as journaled (redacted), plus
    /// the raw predicate while this service has it in memory.
    pub check: Option<(CheckKind, String, Option<String>)>,
    pub delivery: Option<Value>,
    /// The page the tool named (`{tabHandle, url}`), journaled as
    /// `browserActionTarget.requested`.
    pub requested: Option<Value>,
}

impl Lease {
    pub fn new(
        message_id: &str,
        generation: u64,
        tool: &str,
        tool_class: ToolClass,
        outputs: Vec<Output>,
    ) -> Self {
        Self {
            message_id: message_id.into(),
            generation,
            tool: tool.into(),
            tool_class,
            outputs,
            state: LeaseState::Open,
            after_stop: false,
            turn_ended: false,
            report_sequence: None,
            landing: Landing::default(),
            verdict: None,
            check: None,
            delivery: None,
            requested: None,
        }
    }
    /// Whether the report computes the verdict from the artifacts.
    pub fn captures(&self) -> bool {
        self.tool_class == ToolClass::Capture || !self.outputs.is_empty()
    }
    /// The reply every report of a recorded lease gets: the verdict and each
    /// artifact's terminal state as recorded.
    pub fn reply(&self, artifacts: &Artifacts) -> Value {
        let states: Vec<Value> = self
            .outputs
            .iter()
            .filter_map(|o| artifact_state(artifacts, &o.artifact_id))
            .collect();
        json!({"bound": true, "verdict": self.verdict, "artifacts": states})
    }
}

/// `ArtifactStateReply` for a terminal artifact.
pub(super) fn artifact_state(artifacts: &Artifacts, artifact_id: &str) -> Option<Value> {
    match &artifacts.entries.get(artifact_id)?.state {
        State::Ready(info) => Some(
            json!({"artifactId": artifact_id, "state": "ready", "digest": info.digest, "bytes": info.bytes}),
        ),
        State::Unavailable(reason) => {
            Some(json!({"artifactId": artifact_id, "state": "unavailable", "reason": reason}))
        }
        _ => None,
    }
}

/// The page a report's `actionContext` says the action left behind.
pub(super) fn landing(action_context: Option<&Value>) -> Landing {
    let Some(after) = action_context
        .map(|c| &c["after"])
        .filter(|a| a.is_object())
    else {
        return Landing::default();
    };
    Landing {
        target_id: after["targetId"].as_str().map(String::from),
        url: after["url"].as_str().map(String::from),
        from_reply: after["provenance"] == "actionReply",
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

/// Rebuild one conversation's leases from its journal.
pub(super) fn rebuild<'a>(
    agent_cwd: &Path,
    events: impl IntoIterator<Item = &'a Event>,
) -> HashMap<String, Lease> {
    let mut leases: HashMap<String, Lease> = HashMap::new();
    for event in events {
        let d = &event.data;
        let action = d["actionId"].as_str().unwrap_or("");
        match event.kind.as_str() {
            "actionAdmitted" => {
                let outputs = d["outputs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| Output::from_admitted(agent_cwd, action, Some(action), o))
                    .collect();
                let mut lease = Lease::new(
                    d["messageId"].as_str().unwrap_or(""),
                    d["generation"].as_u64().unwrap_or(0),
                    d["tool"].as_str().unwrap_or(""),
                    parse(&d["toolClass"]).unwrap_or(ToolClass::Act),
                    outputs,
                );
                lease.requested = d.get("requested").filter(|r| r.is_object()).cloned();
                leases.insert(action.into(), lease);
            }
            "browserActionTarget" => {
                if let Some(lease) = leases.get_mut(action) {
                    if d["lateReported"] != true {
                        lease.state = LeaseState::Reporting;
                    }
                    lease.report_sequence = Some(event.sequence);
                    lease.after_stop |= d["afterStop"] == true;
                    lease.landing = landing(d.get("observed"));
                }
            }
            "actionObserved" => {
                if let Some(lease) = leases.get_mut(action) {
                    lease.state = if d["lateReported"] == true {
                        LeaseState::LateReported
                    } else {
                        LeaseState::Reported
                    };
                    lease.verdict = parse(&d["verdict"]);
                    lease.check = parse::<CheckKind>(&d["check"]["kind"]).map(|kind| {
                        (
                            kind,
                            d["check"]["predicate"].as_str().unwrap_or("").to_string(),
                            None,
                        )
                    });
                }
            }
            "fileDelivered" => {
                if let Some(lease) = leases.get_mut(action) {
                    lease.delivery = Some(json!({"state": d["state"]}));
                }
            }
            "turnEnded" => {
                // Every lease of the ended turn still open was closed then;
                // `openLeases` is only a bounded display list.
                let message = d["messageId"].as_str().unwrap_or("");
                for lease in leases.values_mut() {
                    if lease.message_id == message {
                        lease.turn_ended = true;
                        if lease.state == LeaseState::Open {
                            lease.state = LeaseState::Unreported;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    leases
}

fn cap(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

/// Compute a `taskAssessment` (plan 4.3) for `message_id` from what the
/// agent declared and the evidence leased under that message. `outcome` is
/// the turn's outcome for the final assessment, `None` while provisional.
pub(super) fn assess(
    declared: &Value,
    message_id: &str,
    leases: &HashMap<String, Lease>,
    artifacts: &Artifacts,
    outcome: Option<&str>,
    source: &str,
) -> Value {
    let phase = if outcome.is_some() {
        "final"
    } else {
        "provisional"
    };
    let mut downgraded: Vec<String> = Vec::new();
    let mut requirements = Vec::new();
    let empty = Vec::new();
    let listed = declared["requirements"].as_array().unwrap_or(&empty);
    if listed.len() > MAX_REQUIREMENTS {
        downgraded.push(format!(
            "only the first {MAX_REQUIREMENTS} requirements were considered"
        ));
    }
    for (index, requirement) in listed.iter().take(MAX_REQUIREMENTS).enumerate() {
        let text = cap(requirement["text"].as_str().unwrap_or(""));
        let status = requirement["status"]
            .as_str()
            .filter(|s| matches!(*s, "met" | "unmet" | "unknown"))
            .unwrap_or("unknown");
        let supplied = requirement.get("check").filter(|c| c.is_object());
        let (computed, because, recorded) = match status {
            "unmet" => ("unmet", "declared unmet".to_string(), None),
            "unknown" => ("unknown", "declared unknown".to_string(), None),
            _ => match supplied {
                None => ("claimed", "no check was named".to_string(), None),
                Some(check) => verify(check, message_id, leases, artifacts),
            },
        };
        if status == "met" && computed != "checked" {
            downgraded.push(format!("requirement {}: {because}", index + 1));
        }
        // The predicate shown is the one the cited action recorded; the
        // agent's own wording is only a label.
        let check = supplied.map(|c| {
            json!({
                "kind": c["kind"].as_str().map(cap),
                "predicate": recorded
                    .clone()
                    .or_else(|| c["predicate"].as_str().map(String::from))
                    .map(|p| cap(&p)),
                "label": c["predicate"].as_str().map(cap),
                "evidence": c["evidence"].as_str().map(cap),
            })
        });
        requirements.push(json!({
            "id": index + 1,
            "text": text,
            "declared": status,
            "check": check,
            "computed": computed,
            "because": cap(&because),
        }));
    }
    let blocker = declared.get("blocker").filter(|b| b.is_object()).map(|b| {
        let kind = b["kind"]
            .as_str()
            .filter(|k| {
                matches!(
                    *k,
                    "humanCheck" | "login" | "permission" | "missingInput" | "siteError" | "other"
                )
            })
            .unwrap_or("other");
        json!({"kind": kind, "text": cap(b["text"].as_str().unwrap_or(""))})
    });
    let checked = requirements
        .iter()
        .filter(|r| r["computed"] == "checked")
        .count();
    let all_checked = !requirements.is_empty() && checked == requirements.len();
    let mut status = if blocker.is_some() {
        "blocked"
    } else if all_checked {
        "checksPassed"
    } else if checked > 0 {
        "someChecksPassed"
    } else {
        "notChecked"
    };
    if status == "checksPassed" && outcome.is_some_and(|o| o != "completed") {
        status = "someChecksPassed";
        downgraded.push(format!(
            "the request ended {}",
            outcome.unwrap_or("without completing")
        ));
    }
    let declared_status = declared["status"].as_str().map(cap);
    json!({
        "v": 1,
        "messageId": message_id,
        "phase": phase,
        "source": source,
        "status": status,
        "declaredStatus": declared_status,
        "reason": declared["reason"].as_str().map(cap),
        "requirements": requirements,
        "downgradedBecause": downgraded,
        "blocker": blocker,
    })
}

/// The final assessment when the agent never assessed.
pub(super) fn not_assessed(message_id: &str) -> Value {
    json!({
        "v": 1,
        "messageId": message_id,
        "phase": "final",
        "source": "host",
        "status": "notChecked",
        "declaredStatus": null,
        "reason": "notAssessed",
        "requirements": [],
        "downgradedBecause": [],
        "blocker": null,
    })
}

/// One requirement's check against the evidence: `(computed, because,
/// recorded predicate)`. For expect, readBack and urlMatch the predicate is
/// the one the cited action recorded; the agent's restatement of it (quotes,
/// "eq" for "==") is not compared.
fn verify(
    check: &Value,
    message_id: &str,
    leases: &HashMap<String, Lease>,
    artifacts: &Artifacts,
) -> (&'static str, String, Option<String>) {
    let evidence = check["evidence"].as_str().unwrap_or("");
    let predicate = check["predicate"].as_str().unwrap_or("");
    let kind = check["kind"].as_str().unwrap_or("");
    if kind == "downloadType" {
        let (computed, because) = download_type(evidence, predicate, message_id, artifacts);
        return (computed, because, None);
    }
    let wanted = match kind {
        "expect" => CheckKind::Expect,
        "readBack" => CheckKind::ReadBack,
        "urlMatch" => CheckKind::UrlMatch,
        _ => {
            return (
                "claimed",
                "this kind of check cannot be verified".into(),
                None,
            )
        }
    };
    let Some(lease) = leases.get(evidence) else {
        return (
            "claimed",
            "the cited action is not one of this conversation's".into(),
            None,
        );
    };
    if lease.message_id != message_id {
        return (
            "claimed",
            "the cited action belongs to another request".into(),
            None,
        );
    }
    let Some((recorded, journaled, _)) = &lease.check else {
        return ("claimed", "the cited action recorded no check".into(), None);
    };
    let shown = Some(journaled.clone());
    if *recorded != wanted {
        return (
            "claimed",
            "the cited action recorded another kind of check".into(),
            shown,
        );
    }
    match lease.verdict {
        Some(Verdict::Confirmed) => {}
        Some(Verdict::Failed) => return ("checkFailed", "the check failed".into(), shown),
        _ => {
            return (
                "claimed",
                "the check did not confirm anything".into(),
                shown,
            )
        }
    }
    if wanted == CheckKind::UrlMatch && !lease.landing.from_reply {
        return (
            "claimed",
            "the landing page was not named by the action's own reply".into(),
            shown,
        );
    }
    // Fresh: no later acting action of this turn on the same page.
    let (Some(sequence), Some(target)) = (lease.report_sequence, &lease.landing.target_id) else {
        return ("checked", "the browser recorded this check".into(), shown);
    };
    let stale = leases.values().any(|other| {
        other.message_id == message_id
            && other.tool_class == ToolClass::Act
            && other.report_sequence.is_some_and(|s| s > sequence)
            && other.landing.target_id.as_ref() == Some(target)
    });
    if stale {
        (
            "stale",
            "a later action changed the same page".into(),
            shown,
        )
    } else {
        ("checked", "the browser recorded this check".into(), shown)
    }
}

fn download_type(
    evidence: &str,
    predicate: &str,
    message_id: &str,
    artifacts: &Artifacts,
) -> (&'static str, String) {
    let Some(entry) = artifacts.entries.get(evidence) else {
        return (
            "claimed",
            "the cited file is not one of this conversation's".into(),
        );
    };
    if entry.message_id.as_deref() != Some(message_id) {
        return (
            "claimed",
            "the cited file belongs to another request".into(),
        );
    }
    if entry.kind != super::artifacts::Kind::Download {
        return ("claimed", "the cited file is not a download".into());
    }
    match &entry.state {
        State::Ready(info) if info.bytes == 0 => ("checkFailed", "the file is empty".into()),
        State::Ready(info) if info.type_matches != Some(true) => (
            "checkFailed",
            "the file's content does not match its extension".into(),
        ),
        State::Ready(info)
            if !predicate.trim().is_empty()
                && info.mime.as_deref().map(str::to_ascii_lowercase)
                    != Some(predicate.trim().to_ascii_lowercase()) =>
        {
            ("checkFailed", "the file is not of the stated type".into())
        }
        State::Ready(_) => ("checked", "a non-empty file of that type was saved".into()),
        State::Unavailable(_) => ("checkFailed", "the file was not kept".into()),
        _ => ("claimed", "the file is not ready".into()),
    }
}
