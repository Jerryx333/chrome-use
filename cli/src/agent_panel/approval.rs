//! Approval replies bound to the request the user saw.
//!
//! The journal holds a redacted projection of each backend request, so a
//! reply can no longer echo the request back. The panel sends a decision
//! from a closed set and the service builds the backend result from the
//! unredacted original it kept in memory. Replies from an older panel, which
//! still echo input, are accepted only when the echo equals the projection
//! journaled for that request, frozen when it was sent.

use serde_json::{json, Map, Value};

const MAX_CUSTOM_ANSWER: usize = 2000;
const MAX_DENY_MESSAGE: usize = 500;

/// A backend request waiting for an answer.
pub(super) struct Pending {
    pub request_id: Value,
    pub method: String,
    /// Unredacted params; never journaled, logged or returned.
    pub original: Value,
    /// A reply was dispatched to the backend (not yet confirmed).
    pub sent: bool,
    pub generation: u64,
    pub revision: u64,
    /// The redacted params journaled in this revision's `request` event.
    pub sent_projection: Value,
    /// This request id has had exactly one revision, so a reply without a
    /// revision cannot belong to an older card.
    pub single_revision: bool,
}

fn ask_user_question(method: &str, original: &Value) -> bool {
    method == "can_use_tool" && original["tool_name"] == "AskUserQuestion"
}

fn refused() -> String {
    "Response does not match the backend request's supported schema".into()
}

/// Build the backend result for a decision sent by this build's panel.
pub(super) fn reply_for_decision(
    method: &str,
    original: &Value,
    decision: &Value,
) -> Result<Value, String> {
    let word = decision.as_str();
    let field = |name: &str| {
        decision
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.get(name))
    };
    let result = match method {
        "can_use_tool" if ask_user_question(method, original) => {
            match (word, given_answers(decision)) {
                (Some("deny"), _) => json!({"behavior":"deny","message":"User declined"}),
                (None, Some(answers)) => {
                    let questions = original["input"]["questions"]
                        .as_array()
                        .ok_or_else(refused)?;
                    let mut built = Map::new();
                    for (question, values) in answers_for(questions, answers, "question", true)? {
                        built.insert(question, values.join(", ").into());
                    }
                    let mut input = original["input"].clone();
                    input["answers"] = Value::Object(built);
                    json!({"behavior":"allow","updatedInput":input})
                }
                _ => return Err(refused()),
            }
        }
        "can_use_tool" => match word {
            Some("allow") => {
                json!({"behavior":"allow","updatedInput":original.get("input").cloned().unwrap_or(json!({}))})
            }
            Some("deny") => json!({"behavior":"deny","message":"User declined"}),
            _ => return Err(refused()),
        },
        "item/tool/requestUserInput" => {
            let answers = given_answers(decision).ok_or_else(refused)?;
            let questions = original["questions"].as_array().ok_or_else(refused)?;
            let mut built = Map::new();
            for (id, values) in answers_for(questions, answers, "id", false)? {
                built.insert(id, json!({"answers":values}));
            }
            json!({"answers":built})
        }
        "session/request_permission" => match (word, field("optionId")) {
            (Some("cancel"), _) => json!({"outcome":{"outcome":"cancelled"}}),
            (None, Some(option)) if option.is_string() => {
                json!({"outcome":{"outcome":"selected","optionId":option}})
            }
            _ => return Err(refused()),
        },
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => match word {
            Some(d @ ("accept" | "acceptForSession" | "decline" | "cancel")) => {
                json!({"decision":d})
            }
            _ => return Err(refused()),
        },
        "mcpServer/elicitation/request" => match word {
            Some("accept") => json!({"action":"accept","content":{}}),
            Some(a @ ("decline" | "cancel")) => json!({"action":a,"content":null}),
            _ => return Err(refused()),
        },
        "item/permissions/requestApproval" => match word {
            Some("accept") => {
                json!({"permissions":original.get("permissions").cloned().unwrap_or(json!({})),"scope":"turn"})
            }
            _ => return Err(refused()),
        },
        _ => return Err(refused()),
    };
    Ok(result)
}

/// Validated answers in the original's question order. `key` names the
/// question identity field (`question` text for Claude, `id` for Codex).
/// Each value is `{option}`, `{options}` (multi-select only) or `{custom}`
/// where free text is allowed.
/// The two answer forms, never mixed: `answersByPosition` (this panel: a
/// list in question order with option indices, because the shown text may
/// be redacted) or `answers` keyed by the original question text or id with
/// original option labels (older form).
enum Answers<'a> {
    Position(&'a [Value]),
    Keyed(&'a Map<String, Value>),
}

fn given_answers(decision: &Value) -> Option<Answers<'_>> {
    let object = decision.as_object().filter(|o| o.len() == 1)?;
    if let Some(list) = object.get("answersByPosition") {
        return list.as_array().map(|l| Answers::Position(l.as_slice()));
    }
    object.get("answers")?.as_object().map(Answers::Keyed)
}

fn answers_for(
    questions: &[Value],
    answers: Answers<'_>,
    key: &str,
    custom_always: bool,
) -> Result<Vec<(String, Vec<String>)>, String> {
    let count = match &answers {
        Answers::Position(list) => list.len(),
        Answers::Keyed(map) => map.len(),
    };
    if count != questions.len() {
        return Err("Answer each question exactly once".into());
    }
    let by_position = matches!(answers, Answers::Position(_));
    let mut out = Vec::new();
    for (index, question) in questions.iter().enumerate() {
        let id = question[key].as_str().ok_or_else(refused)?;
        let answer = match &answers {
            Answers::Position(list) => list.get(index),
            Answers::Keyed(map) => map.get(id),
        }
        .and_then(Value::as_object)
        .filter(|o| o.len() == 1)
        .ok_or_else(refused)?;
        let labels: Vec<&str> = question["options"]
            .as_array()
            .map(|o| o.iter().filter_map(|o| o["label"].as_str()).collect())
            .unwrap_or_default();
        let multi = question["multiSelect"] == true;
        // By position an option is its index in the original; keyed, its
        // original label.
        let pick = |option: &Value| -> Result<String, String> {
            let label = match option {
                Value::Number(n) if by_position => n
                    .as_u64()
                    .and_then(|i| labels.get(i as usize).copied())
                    .ok_or_else(refused)?,
                Value::String(s) if !by_position && labels.contains(&s.as_str()) => s.as_str(),
                _ => return Err(refused()),
            };
            Ok(label.to_string())
        };
        let values = if let Some(option) = answer.get("option") {
            vec![pick(option)?]
        } else if let Some(options) = answer.get("options") {
            let options = options.as_array().ok_or_else(refused)?;
            if !multi || options.is_empty() {
                return Err(refused());
            }
            let mut picked: Vec<String> = Vec::new();
            for option in options {
                let option = pick(option)?;
                if picked.contains(&option) {
                    return Err(refused());
                }
                picked.push(option);
            }
            picked
        } else if let Some(custom) = answer.get("custom") {
            let custom = custom.as_str().ok_or_else(refused)?.trim();
            let allowed = custom_always || question["isOther"] == true || labels.is_empty();
            if !allowed || custom.is_empty() || custom.chars().count() > MAX_CUSTOM_ANSWER {
                return Err(refused());
            }
            vec![custom.to_string()]
        } else {
            return Err(refused());
        };
        out.push((id.to_string(), values));
    }
    Ok(out)
}

fn only_keys(value: &Value, allowed: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str())))
}

/// Build the backend result for a reply from a panel that predates request
/// revisions. Echoed input must equal the frozen projection; the original is
/// then substituted for it.
pub(super) fn reply_for_legacy(
    method: &str,
    original: &Value,
    sent_projection: &Value,
    result: &Value,
) -> Result<Value, String> {
    match method {
        "can_use_tool" => match result["behavior"].as_str() {
            Some("deny") => {
                if !only_keys(result, &["behavior", "message"])
                    || result.get("message").is_some_and(|m| !m.is_string())
                {
                    return Err(refused());
                }
                let message: String = result["message"]
                    .as_str()
                    .unwrap_or("User declined")
                    .chars()
                    .take(MAX_DENY_MESSAGE)
                    .collect();
                Ok(json!({"behavior":"deny","message":message}))
            }
            Some("allow") if only_keys(result, &["behavior", "updatedInput"]) => {
                let echoed = result.get("updatedInput").ok_or_else(refused)?;
                let shown = sent_projection.get("input").cloned().unwrap_or(json!({}));
                if !ask_user_question(method, original) {
                    if echoed != &shown {
                        return Err(refused());
                    }
                    return Ok(
                        json!({"behavior":"allow","updatedInput":original.get("input").cloned().unwrap_or(json!({}))}),
                    );
                }
                let without = |v: &Value| {
                    let mut v = v.clone();
                    if let Some(o) = v.as_object_mut() {
                        o.remove("answers");
                    }
                    v
                };
                if without(echoed) != without(&shown) {
                    return Err(refused());
                }
                let questions = original["input"]["questions"]
                    .as_array()
                    .ok_or_else(refused)?;
                let given = echoed["answers"].as_object().ok_or_else(refused)?;
                let mut decision = Map::new();
                for question in questions {
                    let text = question["question"].as_str().ok_or_else(refused)?;
                    let value = given
                        .get(text)
                        .and_then(Value::as_str)
                        .ok_or_else(refused)?;
                    decision.insert(text.into(), legacy_answer(question, value));
                }
                if given.len() != questions.len() {
                    return Err(refused());
                }
                reply_for_decision(method, original, &json!({"answers":decision}))
            }
            _ => Err(refused()),
        },
        "item/permissions/requestApproval" => {
            if !only_keys(result, &["permissions", "scope"])
                || result.get("permissions")
                    != Some(
                        &sent_projection
                            .get("permissions")
                            .cloned()
                            .unwrap_or(json!({})),
                    )
                || !["turn", "session"].contains(&result["scope"].as_str().unwrap_or(""))
            {
                return Err(refused());
            }
            Ok(
                json!({"permissions":original.get("permissions").cloned().unwrap_or(json!({})),"scope":result["scope"]}),
            )
        }
        // Decisions and option ids carry no request input.
        _ => Ok(result.clone()),
    }
}

/// An older panel sends AskUserQuestion answers as text: one label, labels
/// joined with ", " for multi-select, or free text.
fn legacy_answer(question: &Value, value: &str) -> Value {
    let labels: Vec<&str> = question["options"]
        .as_array()
        .map(|o| o.iter().filter_map(|o| o["label"].as_str()).collect())
        .unwrap_or_default();
    if labels.contains(&value) {
        return json!({"option":value});
    }
    if question["multiSelect"] == true {
        let parts: Vec<&str> = value.split(", ").collect();
        if parts.len() > 1 && parts.iter().all(|p| labels.contains(p)) {
            return json!({"options":parts});
        }
    }
    json!({"custom":value})
}

/// The method's decline or cancel for a request that arrives while the user
/// is stopping a full-access turn. Questions and methods without a decline
/// form are left for the turn's end.
pub(super) fn automatic_decline(method: &str, original: &Value) -> Option<Value> {
    match method {
        "session/request_permission" => Some(
            original["options"]
                .as_array()
                .and_then(|o| o.iter().find(|o| o["kind"] == "reject_once"))
                .map(|o| json!({"outcome":{"outcome":"selected","optionId":o["optionId"]}}))
                .unwrap_or(json!({"outcome":{"outcome":"cancelled"}})),
        ),
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            Some(json!({"decision":"decline"}))
        }
        "mcpServer/elicitation/request" => Some(json!({"action":"decline","content":null})),
        "can_use_tool" if !ask_user_question(method, original) => {
            Some(json!({"behavior":"deny","message":"The user stopped this request"}))
        }
        _ => None,
    }
}

/// Whether a built result declines or cancels the request, the only answers
/// accepted while a conversation is stopping.
pub(super) fn is_decline(method: &str, original: &Value, result: &Value) -> bool {
    match method {
        "can_use_tool" => result["behavior"] == "deny",
        "session/request_permission" => {
            result["outcome"]["outcome"] == "cancelled"
                || (result["outcome"]["outcome"] == "selected"
                    && original["options"].as_array().is_some_and(|options| {
                        options.iter().any(|o| {
                            o["optionId"] == result["outcome"]["optionId"]
                                && o["kind"].as_str().is_some_and(|k| k.starts_with("reject"))
                        })
                    }))
        }
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            ["decline", "cancel"].contains(&result["decision"].as_str().unwrap_or(""))
        }
        "mcpServer/elicitation/request" => {
            ["decline", "cancel"].contains(&result["action"].as_str().unwrap_or(""))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question() -> Value {
        json!({"tool_name":"AskUserQuestion","tool_use_id":"t","input":{"questions":[
            {"question":"Which size?","header":"Size","multiSelect":false,"options":[{"label":"Small"},{"label":"Large"}]},
            {"question":"Which extras?","header":"Extras","multiSelect":true,"options":[{"label":"Bag"},{"label":"Card"}]}
        ],"note":"SYNTH_ORIGINAL"}})
    }

    #[test]
    fn decisions_build_results_from_the_original_only() {
        let original = json!({"tool_name":"mcp__chrome_use__chrome_use_fill","input":{"selector":"#password","value":"SYNTH_SECRET"}});
        assert_eq!(
            reply_for_decision("can_use_tool", &original, &json!("allow")).unwrap(),
            json!({"behavior":"allow","updatedInput":{"selector":"#password","value":"SYNTH_SECRET"}})
        );
        assert_eq!(
            reply_for_decision("can_use_tool", &original, &json!("deny")).unwrap()["behavior"],
            "deny"
        );
        for bad in [
            json!({"updatedInput":{"value":"x"}}),
            json!({"allow":true}),
            json!("accept"),
            json!(null),
            json!({"answers":{}}),
        ] {
            assert!(
                reply_for_decision("can_use_tool", &original, &bad).is_err(),
                "{bad}"
            );
        }
        let codex = json!({"command":"ls"});
        for d in ["accept", "acceptForSession", "decline", "cancel"] {
            assert_eq!(
                reply_for_decision("item/commandExecution/requestApproval", &codex, &json!(d))
                    .unwrap(),
                json!({"decision":d})
            );
        }
        assert!(reply_for_decision(
            "item/fileChange/requestApproval",
            &codex,
            &json!({"decision":"accept"})
        )
        .is_err());
        let acp = json!({"options":[{"optionId":"once","kind":"allow_once"}]});
        assert_eq!(
            reply_for_decision(
                "session/request_permission",
                &acp,
                &json!({"optionId":"once"})
            )
            .unwrap(),
            json!({"outcome":{"outcome":"selected","optionId":"once"}})
        );
        assert_eq!(
            reply_for_decision(
                "item/permissions/requestApproval",
                &json!({"permissions":{"network":true}}),
                &json!("accept")
            )
            .unwrap(),
            json!({"permissions":{"network":true},"scope":"turn"})
        );
    }

    #[test]
    fn ask_user_question_sets_only_validated_answers() {
        let original = question();
        let built = reply_for_decision(
            "can_use_tool",
            &original,
            &json!({"answers":{"Which size?":{"option":"Large"},"Which extras?":{"options":["Bag","Card"]}}}),
        )
        .unwrap();
        let mut expected = original["input"].clone();
        expected["answers"] = json!({"Which size?":"Large","Which extras?":"Bag, Card"});
        assert_eq!(built, json!({"behavior":"allow","updatedInput":expected}));
        let custom = reply_for_decision(
            "can_use_tool",
            &original,
            &json!({"answers":{"Which size?":{"custom":"Medium please"},"Which extras?":{"option":"Bag"}}}),
        )
        .unwrap();
        assert_eq!(
            custom["updatedInput"]["answers"]["Which size?"],
            "Medium please"
        );
        for bad in [
            // unknown question
            json!({"answers":{"Which size?":{"option":"Large"},"Other?":{"option":"Bag"}}}),
            // unknown option
            json!({"answers":{"Which size?":{"option":"Huge"},"Which extras?":{"option":"Bag"}}}),
            // two values for single-select
            json!({"answers":{"Which size?":{"options":["Small","Large"]},"Which extras?":{"option":"Bag"}}}),
            // missing question
            json!({"answers":{"Which size?":{"option":"Large"}}}),
            // two shapes in one answer
            json!({"answers":{"Which size?":{"option":"Large","custom":"x"},"Which extras?":{"option":"Bag"}}}),
            // oversized custom answer
            json!({"answers":{"Which size?":{"custom":"x".repeat(2001)},"Which extras?":{"option":"Bag"}}}),
            // client data merged into input
            json!({"answers":{"Which size?":{"option":"Large"},"Which extras?":{"option":"Bag"}},"updatedInput":{}}),
        ] {
            assert!(
                reply_for_decision("can_use_tool", &original, &bad).is_err(),
                "{bad}"
            );
        }
        // Codex questions allow free text only where the question says so.
        let codex = json!({"questions":[
            {"id":"q1","question":"Pick","isOther":false,"options":[{"label":"A"}]},
            {"id":"q2","question":"Name","isOther":false,"options":null}
        ]});
        assert_eq!(
            reply_for_decision(
                "item/tool/requestUserInput",
                &codex,
                &json!({"answers":{"q1":{"option":"A"},"q2":{"custom":"Ada"}}})
            )
            .unwrap(),
            json!({"answers":{"q1":{"answers":["A"]},"q2":{"answers":["Ada"]}}})
        );
        assert!(reply_for_decision(
            "item/tool/requestUserInput",
            &codex,
            &json!({"answers":{"q1":{"custom":"B"},"q2":{"custom":"Ada"}}})
        )
        .is_err());
    }

    #[test]
    fn answers_by_position_map_to_the_original_text() {
        // The panel showed redacted text; the agent must get the original.
        let original = json!({"tool_name":"AskUserQuestion","input":{"questions":[
            {"question":"Use https://x.example/?token=SYNTH_Q?","multiSelect":false,"options":[{"label":"Keep SYNTH_OPT"},{"label":"Drop"}]},
            {"question":"Which?","multiSelect":true,"options":[{"label":"A"},{"label":"B"}]}
        ]}});
        let built = reply_for_decision(
            "can_use_tool",
            &original,
            &json!({"answersByPosition":[{"option":0},{"options":[1,0]}]}),
        )
        .unwrap();
        assert_eq!(
            built["updatedInput"]["answers"],
            json!({"Use https://x.example/?token=SYNTH_Q?":"Keep SYNTH_OPT","Which?":"B, A"})
        );
        for bad in [
            json!({"answersByPosition":[{"option":2},{"option":0}]}),
            json!({"answersByPosition":[{"option":"Keep [redacted]"},{"option":0}]}),
            // Labels are not positions, and positions are not labels.
            json!({"answersByPosition":[{"option":"Keep SYNTH_OPT"},{"option":0}]}),
            json!({"answersByPosition":[{"option":-1},{"option":0}]}),
            json!({"answersByPosition":[{"options":[0]},{"option":0}]}),
            json!({"answersByPosition":[{"option":0},{"options":[0,0]}]}),
            json!({"answersByPosition":[{"option":0}]}),
            // The two forms never mix.
            json!({"answersByPosition":[{"option":0},{"option":0}],"answers":{}}),
            json!({"answers":{"0":{"option":0},"1":{"option":0}}}),
        ] {
            assert!(
                reply_for_decision("can_use_tool", &original, &bad).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn keyed_answers_follow_the_original_ids_even_when_they_look_like_positions() {
        // Codex question ids "1" then "0": keyed answers go by id, never by
        // position, so they cannot be swapped.
        let codex = json!({"questions":[
            {"id":"1","question":"First","isOther":false,"options":[{"label":"Yes"},{"label":"No"}]},
            {"id":"0","question":"Second","isOther":false,"options":[{"label":"Yes"},{"label":"No"}]}
        ]});
        let built = reply_for_decision(
            "item/tool/requestUserInput",
            &codex,
            &json!({"answers":{"1":{"option":"Yes"},"0":{"option":"No"}}}),
        )
        .unwrap();
        assert_eq!(
            built,
            json!({"answers":{"1":{"answers":["Yes"]},"0":{"answers":["No"]}}})
        );
        let by_position = reply_for_decision(
            "item/tool/requestUserInput",
            &codex,
            &json!({"answersByPosition":[{"option":0},{"option":1}]}),
        )
        .unwrap();
        assert_eq!(
            by_position,
            json!({"answers":{"1":{"answers":["Yes"]},"0":{"answers":["No"]}}})
        );
        // Claude questions whose text is a number behave the same.
        let claude = json!({"tool_name":"AskUserQuestion","input":{"questions":[
            {"question":"1","multiSelect":false,"options":[{"label":"Yes"},{"label":"No"}]},
            {"question":"0","multiSelect":false,"options":[{"label":"Yes"},{"label":"No"}]}
        ]}});
        let built = reply_for_decision(
            "can_use_tool",
            &claude,
            &json!({"answers":{"1":{"option":"Yes"},"0":{"option":"No"}}}),
        )
        .unwrap();
        assert_eq!(
            built["updatedInput"]["answers"],
            json!({"1":"Yes","0":"No"})
        );
    }

    #[test]
    fn legacy_echoes_must_equal_the_frozen_projection() {
        let original = json!({"tool_name":"mcp__chrome_use__chrome_use_fill","input":{"selector":"#password","value":"SYNTH_SECRET"}});
        let frozen = json!({"tool_name":"mcp__chrome_use__chrome_use_fill","input":{"selector":"#password","value":"[redacted input, 12 chars]"}});
        let echo = json!({"behavior":"allow","updatedInput":frozen["input"]});
        assert_eq!(
            reply_for_legacy("can_use_tool", &original, &frozen, &echo).unwrap(),
            json!({"behavior":"allow","updatedInput":original["input"]})
        );
        // A projection recomputed later (the secret set grew) differs from
        // the frozen one and is refused.
        let recomputed = json!({"behavior":"allow","updatedInput":{"selector":"[redacted secret, 9 chars]","value":"[redacted input, 12 chars]"}});
        assert!(reply_for_legacy("can_use_tool", &original, &frozen, &recomputed).is_err());
        for bad in [
            json!({"behavior":"allow","updatedInput":{"selector":"#other","value":"x"}}),
            json!({"behavior":"allow"}),
            json!({"behavior":"allow","updatedInput":frozen["input"],"updatedPermissions":[]}),
            json!({"behavior":"maybe"}),
        ] {
            assert!(
                reply_for_legacy("can_use_tool", &original, &frozen, &bad).is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            reply_for_legacy(
                "can_use_tool",
                &original,
                &frozen,
                &json!({"behavior":"deny","message":"User declined"})
            )
            .unwrap()["behavior"],
            "deny"
        );
        // AskUserQuestion: the old panel spreads the input and adds answers.
        let original = question();
        let mut frozen = original.clone();
        frozen["input"]["note"] = "[redacted secret, 14 chars]".into();
        let mut spread = frozen["input"].clone();
        spread["answers"] = json!({"Which size?":"Small","Which extras?":"Bag, Card"});
        let built = reply_for_legacy(
            "can_use_tool",
            &original,
            &frozen,
            &json!({"behavior":"allow","updatedInput":spread}),
        )
        .unwrap();
        assert_eq!(built["updatedInput"]["note"], "SYNTH_ORIGINAL");
        assert_eq!(
            built["updatedInput"]["answers"],
            json!({"Which size?":"Small","Which extras?":"Bag, Card"})
        );
        let mut tampered = spread.clone();
        tampered["note"] = "changed".into();
        assert!(reply_for_legacy(
            "can_use_tool",
            &original,
            &frozen,
            &json!({"behavior":"allow","updatedInput":tampered})
        )
        .is_err());
        // Permissions: the echo must equal the projection; the original is used.
        let original = json!({"permissions":{"fileSystem":{"write":["/SYNTH_PATH"]}}});
        let frozen = json!({"permissions":{"fileSystem":{"write":["[redacted]"]}}});
        assert_eq!(
            reply_for_legacy(
                "item/permissions/requestApproval",
                &original,
                &frozen,
                &json!({"permissions":frozen["permissions"],"scope":"turn"})
            )
            .unwrap(),
            json!({"permissions":original["permissions"],"scope":"turn"})
        );
        assert!(reply_for_legacy(
            "item/permissions/requestApproval",
            &original,
            &frozen,
            &json!({"permissions":original["permissions"],"scope":"turn"})
        )
        .is_err());
    }

    #[test]
    fn declines_are_recognized_per_method() {
        let acp = json!({"options":[{"optionId":"no","kind":"reject_once"},{"optionId":"yes","kind":"allow_once"}]});
        let decline = automatic_decline("session/request_permission", &acp).unwrap();
        assert!(is_decline("session/request_permission", &acp, &decline));
        assert!(!is_decline(
            "session/request_permission",
            &acp,
            &json!({"outcome":{"outcome":"selected","optionId":"yes"}})
        ));
        for method in [
            "item/commandExecution/requestApproval",
            "item/fileChange/requestApproval",
            "mcpServer/elicitation/request",
            "can_use_tool",
        ] {
            let decline = automatic_decline(method, &json!({})).unwrap();
            assert!(is_decline(method, &json!({}), &decline), "{method}");
        }
        assert!(automatic_decline("can_use_tool", &question()).is_none());
        assert!(automatic_decline("item/permissions/requestApproval", &json!({})).is_none());
        assert!(!is_decline(
            "item/permissions/requestApproval",
            &json!({}),
            &json!({"permissions":{},"scope":"turn"})
        ));
    }
}
