//! What a phone may ask for. Only the operations below exist from the phone;
//! everything else is refused here and never reaches the panel service.
//! Parameters are checked field by field: a field the phone may not set is
//! refused, never silently dropped, except `fullAccess:false` on create.

use serde_json::{json, Map, Value};

#[derive(Debug, PartialEq)]
pub enum Action {
    /// Forward to the panel service under the device's profile.
    Panel {
        operation: &'static str,
        params: Value,
    },
    /// `conversationSetOptions`, after checking it does not raise tool access.
    SetOptions {
        conversation_id: String,
        options: Map<String, Value>,
    },
    /// `scheduleSave` reduced to `{id, enabled}` of an existing schedule.
    ScheduleToggle {
        id: String,
        enabled: bool,
    },
    Subscribe {
        conversation_id: String,
        after: u64,
    },
    Unsubscribe {
        conversation_id: String,
    },
    PushRegister {
        relay_token: String,
    },
    PushUnregister,
    Screenshot {
        conversation_id: String,
    },
}

#[derive(Debug, PartialEq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

fn denied(message: impl Into<String>) -> Refusal {
    Refusal {
        code: "denied",
        message: message.into(),
    }
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal {
        code: "invalidParams",
        message: message.into(),
    }
}

/// Tool access, ordered from least to most authority.
pub fn access_rank(access: &str) -> Option<u8> {
    match access {
        "browserOnly" => Some(0),
        "readFiles" => Some(1),
        "writeFiles" => Some(2),
        "runCommands" => Some(3),
        _ => None,
    }
}

/// The access a conversation effectively runs with; mirrors
/// `ModelOptions::effective_access` in the panel service.
pub fn effective_rank(agent: &str, tool_access: Option<&str>, full_access: bool) -> u8 {
    match tool_access.and_then(access_rank) {
        Some(rank) => rank,
        None if full_access || agent == "opencode" => 3,
        None if agent == "codex" => 1,
        None => 0,
    }
}

/// The highest tool access a conversation created from the phone may have.
pub const PHONE_CREATE_MAX_RANK: u8 = 2;

fn fields<'a>(
    params: &'a Value,
    allowed: &[&str],
    required: &[&str],
    op: &str,
) -> Result<&'a Map<String, Value>, Refusal> {
    let map = params
        .as_object()
        .ok_or_else(|| invalid(format!("{op} params must be an object")))?;
    if let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(denied(format!(
            "{op} does not accept `{key}` from the phone"
        )));
    }
    if let Some(key) = required.iter().find(|k| !map.contains_key(**k)) {
        return Err(invalid(format!("{op} requires `{key}`")));
    }
    Ok(map)
}

fn conversation_id(map: &Map<String, Value>) -> Result<String, Refusal> {
    map.get("conversationId")
        .and_then(Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(str::to_owned)
        .ok_or_else(|| invalid("conversationId must be a conversation UUID"))
}

fn check_options(options: &Value) -> Result<&Map<String, Value>, Refusal> {
    let map = options
        .as_object()
        .ok_or_else(|| invalid("options must be an object"))?;
    if let Some(key) = map
        .keys()
        .find(|k| !["effort", "serviceTier", "toolAccess"].contains(&k.as_str()))
    {
        return Err(invalid(format!("Unknown conversation option `{key}`")));
    }
    if let Some(access) = map.get("toolAccess").filter(|v| !v.is_null()) {
        access
            .as_str()
            .and_then(access_rank)
            .ok_or_else(|| invalid("Unknown toolAccess value"))?;
    }
    Ok(map)
}

pub fn check(op: &str, params: &Value) -> Result<Action, Refusal> {
    let forward = |operation: &'static str, params: Value| Ok(Action::Panel { operation, params });
    match op {
        "discover" | "conversationList" | "scheduleList" => {
            fields(params, &[], &[], op)?;
            let operation = match op {
                "discover" => "discover",
                "conversationList" => "conversationList",
                _ => "scheduleList",
            };
            forward(operation, json!({}))
        }
        "conversationCreate" => {
            let map = fields(
                params,
                &["agentId", "model", "options", "fullAccess"],
                &["agentId"],
                op,
            )?;
            if map.get("fullAccess").is_some_and(|v| v != &json!(false)) {
                return Err(denied("Full access cannot be granted from the phone"));
            }
            let agent = map["agentId"]
                .as_str()
                .ok_or_else(|| invalid("agentId must be a string"))?;
            let mut out = Map::new();
            out.insert("agentId".into(), agent.into());
            if let Some(model) = map.get("model").filter(|v| !v.is_null()) {
                let model = model
                    .as_str()
                    .ok_or_else(|| invalid("model must be a string"))?;
                out.insert("model".into(), model.into());
            }
            let options = match map.get("options") {
                Some(options) => check_options(options)?.clone(),
                None => Map::new(),
            };
            let rank = effective_rank(
                agent,
                options.get("toolAccess").and_then(Value::as_str),
                false,
            );
            if rank > PHONE_CREATE_MAX_RANK {
                return Err(denied(
                    "A conversation created from the phone cannot run commands; choose writeFiles or less",
                ));
            }
            out.insert("options".into(), Value::Object(options));
            out.insert("fullAccess".into(), false.into());
            forward("conversationCreate", Value::Object(out))
        }
        "conversationSend" | "conversationSendDirect" => {
            let map = fields(
                params,
                &[
                    "conversationId",
                    "messageId",
                    "text",
                    "attachments",
                    "record",
                ],
                &["conversationId", "messageId", "text"],
                op,
            )?;
            conversation_id(map)?;
            if map.get("record").is_some_and(|v| v != &json!(false)) {
                return Err(denied("Recording cannot be allowed from the phone"));
            }
            if let Some(attachments) = map.get("attachments") {
                let list = attachments
                    .as_array()
                    .ok_or_else(|| invalid("attachments must be a list"))?;
                let (count, bytes) = crate::agent_panel::ATTACHMENT_LIMITS;
                if list.len() > count {
                    return Err(invalid(format!("A message supports up to {count} images")));
                }
                for item in list {
                    let size = item["bytes"].as_u64().unwrap_or(u64::MAX);
                    if size > bytes as u64 {
                        return Err(invalid("An attachment exceeds 512 KiB"));
                    }
                }
            }
            let mut out = map.clone();
            out.remove("record");
            let operation = if op == "conversationSend" {
                "conversationSend"
            } else {
                "conversationSendDirect"
            };
            forward(operation, Value::Object(out))
        }
        "conversationStop" => {
            let map = fields(params, &["conversationId"], &["conversationId"], op)?;
            forward(
                "conversationStop",
                json!({"conversationId": conversation_id(map)?}),
            )
        }
        "conversationRespond" => {
            let keys = [
                "conversationId",
                "requestId",
                "generation",
                "requestRevision",
                "decision",
            ];
            let map = fields(params, &keys, &keys, op)?;
            conversation_id(map)?;
            forward("conversationRespond", Value::Object(map.clone()))
        }
        "conversationRead" => {
            let map = fields(
                params,
                &["conversationId", "after"],
                &["conversationId"],
                op,
            )?;
            let id = conversation_id(map)?;
            let after = match map.get("after") {
                Some(v) => v
                    .as_u64()
                    .ok_or_else(|| invalid("after must be a cursor number"))?,
                None => 0,
            };
            forward(
                "conversationRead",
                json!({"conversationId": id, "after": after}),
            )
        }
        "conversationAttachmentPut" => {
            let map = fields(
                params,
                &["conversationId", "data"],
                &["conversationId", "data"],
                op,
            )?;
            let id = conversation_id(map)?;
            let data = map["data"]
                .as_str()
                .ok_or_else(|| invalid("data must be base64 image bytes"))?;
            let (_, bytes) = crate::agent_panel::ATTACHMENT_LIMITS;
            if data.len() > bytes.div_ceil(3) * 4 {
                return Err(invalid("Image exceeds 512 KiB; resize it before attaching"));
            }
            forward(
                "conversationAttachmentPut",
                json!({"conversationId": id, "data": data}),
            )
        }
        "conversationDelete" => {
            let map = fields(
                params,
                &["conversationId", "confirm"],
                &["conversationId", "confirm"],
                op,
            )?;
            if map["confirm"] != json!(true) {
                return Err(invalid("Deleting a conversation requires confirm: true"));
            }
            forward(
                "conversationDelete",
                json!({"conversationId": conversation_id(map)?}),
            )
        }
        "conversationSetOptions" => {
            let map = fields(
                params,
                &["conversationId", "options"],
                &["conversationId", "options"],
                op,
            )?;
            Ok(Action::SetOptions {
                conversation_id: conversation_id(map)?,
                options: check_options(&map["options"])?.clone(),
            })
        }
        "scheduleRun" => {
            let map = fields(params, &["id"], &["id"], op)?;
            forward("scheduleRun", json!({"id": schedule_id(map)?}))
        }
        "scheduleSave" => {
            let map = fields(params, &["id", "enabled"], &["id", "enabled"], op)?;
            Ok(Action::ScheduleToggle {
                id: schedule_id(map)?,
                enabled: map["enabled"]
                    .as_bool()
                    .ok_or_else(|| invalid("enabled must be true or false"))?,
            })
        }
        "subscribe" => {
            let map = fields(
                params,
                &["conversationId", "after"],
                &["conversationId"],
                op,
            )?;
            Ok(Action::Subscribe {
                conversation_id: conversation_id(map)?,
                after: match map.get("after") {
                    Some(v) => v
                        .as_u64()
                        .ok_or_else(|| invalid("after must be a cursor number"))?,
                    None => 0,
                },
            })
        }
        "unsubscribe" => {
            let map = fields(params, &["conversationId"], &["conversationId"], op)?;
            Ok(Action::Unsubscribe {
                conversation_id: conversation_id(map)?,
            })
        }
        "pushRegister" => {
            let map = fields(params, &["relayToken"], &["relayToken"], op)?;
            let token = map["relayToken"]
                .as_str()
                .filter(|t| {
                    !t.is_empty()
                        && t.len() <= 2048
                        && t.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.=".contains(&b))
                })
                .ok_or_else(|| invalid("relayToken is not a relay token"))?;
            Ok(Action::PushRegister {
                relay_token: token.into(),
            })
        }
        "pushUnregister" => {
            fields(params, &[], &[], op)?;
            Ok(Action::PushUnregister)
        }
        "screenshot" => {
            let map = fields(params, &["conversationId"], &["conversationId"], op)?;
            Ok(Action::Screenshot {
                conversation_id: conversation_id(map)?,
            })
        }
        _ => Err(denied(format!("{op} is not available from the phone"))),
    }
}

fn schedule_id(map: &Map<String, Value>) -> Result<String, Refusal> {
    map.get("id")
        .and_then(Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(str::to_owned)
        .ok_or_else(|| invalid("id must be a schedule UUID"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e";

    fn code(op: &str, params: Value) -> &'static str {
        check(op, &params).unwrap_err().code
    }

    #[test]
    fn operations_outside_the_allowlist_are_denied() {
        for op in [
            "conversationSetAccess",
            "conversationProjectChoose",
            "conversationProjectList",
            "conversationResume",
            "conversationSessionList",
            "conversationTabGrant",
            "conversationSkillSelect",
            "conversationRecoveryReview",
            "conversationRecoveryTerminate",
            "conversationRecoveryFork",
            "conversationArtifactRead",
            "conversationAttachmentRead",
            "conversationQueueEdit",
            "serviceStop",
            "serviceHealth",
            "health",
            "skillSave",
            "skillImport",
            "skillSetEnabled",
            "scheduleDelete",
            "agentScheduleSave",
            "configRead",
            "configApply",
            "configRestore",
            "imageConvert",
            "inspectList",
            "",
        ] {
            assert_eq!(code(op, json!({})), "denied", "{op}");
        }
    }

    #[test]
    fn create_never_grants_full_access_or_commands() {
        assert_eq!(
            code(
                "conversationCreate",
                json!({"agentId":"claude","fullAccess":true})
            ),
            "denied"
        );
        for field in ["executable", "project"] {
            let mut params = json!({"agentId":"claude"});
            params[field] = json!("/tmp/x");
            assert_eq!(code("conversationCreate", params), "denied", "{field}");
        }
        assert_eq!(
            code(
                "conversationCreate",
                json!({"agentId":"codex","options":{"toolAccess":"runCommands"}})
            ),
            "denied"
        );
        // OpenCode always runs commands, so it is not created from the phone.
        assert_eq!(
            code("conversationCreate", json!({"agentId":"opencode"})),
            "denied"
        );
        let ok = check(
            "conversationCreate",
            &json!({"agentId":"codex","model":"gpt","fullAccess":false,"options":{"toolAccess":"writeFiles"}}),
        )
        .unwrap();
        assert_eq!(
            ok,
            Action::Panel {
                operation: "conversationCreate",
                params: json!({"agentId":"codex","model":"gpt","options":{"toolAccess":"writeFiles"},"fullAccess":false})
            }
        );
        let Action::Panel { params, .. } =
            check("conversationCreate", &json!({"agentId":"claude"})).unwrap()
        else {
            panic!()
        };
        assert_eq!(params["fullAccess"], false);
    }

    #[test]
    fn send_refuses_desktop_only_fields_and_checks_attachments() {
        let base = json!({"conversationId":ID,"messageId":ID,"text":"hi"});
        assert!(check("conversationSend", &base).is_ok());
        for (field, value) in [
            ("page", json!({"url":"https://a"})),
            ("tabGrant", json!(null)),
            ("skillId", json!("s")),
            ("record", json!(true)),
        ] {
            let mut params = base.clone();
            params[field] = value;
            assert_eq!(code("conversationSendDirect", params), "denied", "{field}");
        }
        let image = json!({"id":"a".repeat(64),"mediaType":"image/png","bytes":10});
        let mut params = base.clone();
        params["attachments"] = json!(vec![image.clone(); 6]);
        assert_eq!(code("conversationSend", params.clone()), "invalidParams");
        params["attachments"] = json!(vec![image; 5]);
        assert!(check("conversationSend", &params).is_ok());
        params["attachments"] = json!([{"id":"a","mediaType":"image/png","bytes":600000}]);
        assert_eq!(code("conversationSend", params), "invalidParams");
        assert_eq!(
            code(
                "conversationSend",
                json!({"conversationId":"x","messageId":ID,"text":"t"})
            ),
            "invalidParams"
        );
    }

    #[test]
    fn respond_needs_a_revision_bound_decision() {
        let ok = json!({"conversationId":ID,"requestId":3,"generation":1,"requestRevision":2,"decision":"allow"});
        assert!(check("conversationRespond", &ok).is_ok());
        let mut legacy = ok.clone();
        legacy["result"] = json!({"behavior":"allow"});
        assert_eq!(code("conversationRespond", legacy), "denied");
        let mut missing = ok;
        missing.as_object_mut().unwrap().remove("requestRevision");
        assert_eq!(code("conversationRespond", missing), "invalidParams");
    }

    #[test]
    fn delete_attachment_and_schedule_limits() {
        assert_eq!(
            code("conversationDelete", json!({"conversationId":ID})),
            "invalidParams"
        );
        assert_eq!(
            code(
                "conversationDelete",
                json!({"conversationId":ID,"confirm":"yes"})
            ),
            "invalidParams"
        );
        assert_eq!(
            check(
                "conversationDelete",
                &json!({"conversationId":ID,"confirm":true})
            )
            .unwrap(),
            Action::Panel {
                operation: "conversationDelete",
                params: json!({"conversationId":ID})
            }
        );
        let big = "A".repeat(700_000);
        assert_eq!(
            code(
                "conversationAttachmentPut",
                json!({"conversationId":ID,"data":big})
            ),
            "invalidParams"
        );
        assert_eq!(
            code(
                "conversationAttachmentPut",
                json!({"conversationId":ID,"data":"","attachment":{}})
            ),
            "denied"
        );
        for extra in [
            "name",
            "prompt",
            "rule",
            "options",
            "fullAccess",
            "acknowledgeUnconfirmed",
        ] {
            let mut params = json!({"id":ID,"enabled":true});
            params[extra] = json!("x");
            assert_eq!(code("scheduleSave", params), "denied", "{extra}");
        }
        assert_eq!(code("scheduleSave", json!({"id":ID})), "invalidParams");
        assert_eq!(
            check("scheduleSave", &json!({"id":ID,"enabled":false})).unwrap(),
            Action::ScheduleToggle {
                id: ID.into(),
                enabled: false
            }
        );
    }

    #[test]
    fn set_options_and_gateway_ops() {
        assert_eq!(
            code(
                "conversationSetOptions",
                json!({"conversationId":ID,"options":{"toolAccess":"root"}})
            ),
            "invalidParams"
        );
        assert!(matches!(
            check(
                "conversationSetOptions",
                &json!({"conversationId":ID,"options":{"effort":"high"}})
            ),
            Ok(Action::SetOptions { .. })
        ));
        assert_eq!(
            check("subscribe", &json!({"conversationId":ID,"after":42})).unwrap(),
            Action::Subscribe {
                conversation_id: ID.into(),
                after: 42
            }
        );
        assert_eq!(
            code("pushRegister", json!({"relayToken":"bad token"})),
            "invalidParams"
        );
        assert!(check("pushRegister", &json!({"relayToken":"abc.DEF_-="})).is_ok());
        assert_eq!(code("discover", json!({"x":1})), "denied");
        assert_eq!(code("conversationList", json!([])), "invalidParams");
    }

    #[test]
    fn effective_access_mirrors_the_panel_defaults() {
        assert_eq!(effective_rank("claude", None, false), 0);
        assert_eq!(effective_rank("codex", None, false), 1);
        assert_eq!(effective_rank("opencode", None, false), 3);
        assert_eq!(effective_rank("claude", None, true), 3);
        assert_eq!(effective_rank("claude", Some("writeFiles"), true), 2);
    }
}
