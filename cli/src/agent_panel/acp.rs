//! OpenCode ACP v1 adapter. A private server belongs to this process group;
//! only the prompt response confirms a turn's terminal outcome.
use super::backend::{
    compacted, emit_text, notice, opencode_tool_notice, send_large, write, Command, Notice, Setup,
};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientInfo":{"name":"chrome_use_panel","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}})
}
async fn rpc(
    input: &mut tokio::process::ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    write(
        input,
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await
}
async fn emit(
    events: &mpsc::Sender<Notice>,
    kind: &'static str,
    data: Value,
) -> Result<(), String> {
    events
        .send(notice(kind, data))
        .await
        .map_err(|_| "Event consumer closed".into())
}
fn model(result: &Value) -> Value {
    result["configOptions"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["id"] == "model"))
        .map(|option| option["currentValue"].clone())
        .unwrap_or(Value::Null)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn protocol(
    setup: &Setup,
    commands: &mut mpsc::Receiver<Command>,
    events: &mpsc::Sender<Notice>,
    input: &mut tokio::process::ChildStdin,
    reader: &mut BufReader<tokio::process::ChildStdout>,
    pid: u32,
    browser_env: Value,
    instructions: &str,
    process: Value,
) -> Result<(), String> {
    write(input, initialize()).await?;
    let env: Vec<Value> = browser_env
        .as_object()
        .ok_or("Browser environment is not a map")?
        .iter()
        .map(|(name, value)| json!({"name":name,"value":value}))
        .collect();
    // The project, if any: OpenCode also restores a session only for the
    // directory it was started in.
    let cwd = setup.project.as_ref().unwrap_or(&setup.workspace);
    let mut session_params = json!({"cwd":cwd,"mcpServers":[{"name":"chrome_use","command":setup.mcp_executable,"args":["mcp","--tools","all"],"env":env}]});
    let mut session = setup.backend_session.clone();
    let mut requests = HashMap::from([(1u64, "initialize".to_string())]);
    let mut next_id = 1u64;
    let mut images_supported = false;
    let mut backend_version = Value::Null;
    let mut resumable = false;
    let mut ready = false;
    let mut queued = None;
    let mut active: Option<u64> = None;
    let mut turn_count = 0u64;
    let mut text_item = String::new();
    let mut tools = HashMap::<String, Value>::new();
    let mut selected_model = Value::Null;
    let mut permissions = HashMap::<String, (Value, Value, bool)>::new();
    let mut stopping_at = None;
    let started = tokio::time::Instant::now();
    let mut bytes = Vec::new();
    loop {
        let mut bounded = (&mut *reader).take(16 * 1024 * 1024 + 1 - bytes.len() as u64);
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Start(message)) => {
                    if active.is_some() || queued.is_some() { return Err("Backend received overlapping turns".into()); }
                    queued = Some(message);
                }
                Some(Command::Interrupt) => {
                    if queued.take().is_some() {
                        emit(events,"terminal",json!({"status":"interrupted","error":null})).await?;
                    } else if active.is_some() {
                        write(input,json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})).await?;
                        for (_, (id, _, _)) in permissions.drain() {
                            write(input,json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}})).await?;
                            emit(events,"requestResolved",json!({"requestId":id})).await?;
                        }
                        stopping_at = Some(tokio::time::Instant::now());
                    }
                }
                Some(Command::Reply{id,result}) => {
                    if let Some((_, _, sent)) = permissions.get_mut(&id.to_string()) {
                        if !*sent {
                            write(input,json!({"jsonrpc":"2.0","id":id,"result":result})).await?;
                            *sent = true;
                            emit(events,"replySent",json!({"requestId":id})).await?;
                        }
                    }
                }
                Some(Command::Shutdown) | None => return Ok(()),
            },
            read = bounded.read_until(b'\n', &mut bytes) => {
                let count = read.map_err(|_| "Cannot read ACP output")?;
                if count == 0 { return Err("ACP output closed; an unfinished turn is unconfirmed".into()); }
                if bytes.len() > 16 * 1024 * 1024 || !bytes.ends_with(b"\n") { return Err("ACP emitted an oversized or incomplete protocol line".into()); }
                let value: Value = serde_json::from_slice(&bytes).map_err(|_| "ACP emitted non-protocol output")?;
                bytes.clear();
                if let Some(method) = value["method"].as_str() {
                    let params = &value["params"];
                    if let Some(id) = value.get("id") {
                        if method != "session/request_permission" || params["sessionId"].as_str() != session.as_deref() || active.is_none() {
                            write(input,json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported host operation"}})).await?;
                            continue;
                        }
                        if stopping_at.is_some() {
                            write(input,json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}})).await?;
                        } else {
                            permissions.insert(id.to_string(),(id.clone(),params["toolCall"]["toolCallId"].clone(),false));
                            send_large(events,notice("request",json!({"requestId":id,"method":method,"params":params}))).await?;
                        }
                    } else if method == "session/update" && ready && active.is_some() && params["sessionId"].as_str() == session.as_deref() {
                        let update = &params["update"];
                        match update["sessionUpdate"].as_str() {
                            Some("agent_message_chunk") if update["content"]["type"] == "text" => {
                                if let Some(text) = update["content"]["text"].as_str() { emit_text(events,"textDelta",&text_item,text).await?; }
                            }
                            Some("tool_call" | "tool_call_update") => {
                                let tool_id = update["toolCallId"].as_str().filter(|id| !id.is_empty() && id.len() <= 1024).ok_or("ACP tool call has no bounded identity")?;
                                if tools.len() >= 256 && !tools.contains_key(tool_id) { return Err("ACP tool count exceeded its bound".into()); }
                                let item = tools.entry(tool_id.into()).or_insert_with(|| json!({}));
                                for (key,value) in update.as_object().ok_or("ACP tool update is not an object")? { item[key] = value.clone(); }
                                item["id"] = json!(tool_id);
                                item["type"] = json!("acpToolCall");
                                item["name"] = item["title"].clone();
                                item["input"] = item["rawInput"].clone();
                                if item.get("rawOutput").is_some() { item["input"] = item["rawOutput"].clone(); }
                                else if item.get("content").is_some() { item["input"] = item["content"].clone(); }
                                let completed = ["completed","failed"].contains(&item["status"].as_str().unwrap_or(""));
                                send_large(events,opencode_tool_notice(completed,item)).await?;
                                if completed && update["toolCallId"].is_string() {
                                    let resolved: Vec<_> = permissions.iter().filter(|(_, (_, tool, sent))| *sent && *tool == update["toolCallId"]).map(|(key, (id, _, _))| (key.clone(),id.clone())).collect();
                                    for (key,id) in resolved { permissions.remove(&key); emit(events,"requestResolved",json!({"requestId":id})).await?; }
                                }
                            }
                            Some("session_info_update") => {
                                if let Some(title) = update["title"].as_str() { emit(events,"title",json!({"title":title})).await?; }
                                let compaction = &update["_meta"]["opencode/compaction"];
                                if compaction["status"] == "completed" { events.send(compacted("opencode",compaction["reason"].clone())).await.map_err(|_| "Event consumer closed")?; }
                            }
                            _ => {}
                        }
                    }
                } else if let Some(id) = value["id"].as_u64() {
                    let Some(method) = requests.remove(&id) else { continue; };
                    if value.get("error").is_some() {
                        if method == "session/prompt" {
                            active = None; stopping_at = None;
                            for (_, (id, _, _)) in permissions.drain() { emit(events,"requestResolved",json!({"requestId":id})).await?; }
                            let error = &value["error"];
                            let detail = error["data"]["message"].as_str().or_else(|| error["message"].as_str()).filter(|m| !m.trim().is_empty()).map(|m| m.chars().take(2000).collect::<String>());
                            let message = match detail { Some(detail) => format!("OpenCode refused the prompt: {detail}"), None => "OpenCode refused the prompt".into() };
                            emit(events,"terminal",json!({"status":"failed","error":message})).await?;
                            continue;
                        }
                        return Err(format!("OpenCode refused {method}; no successful outcome was assumed"));
                    }
                    let result = &value["result"];
                    match method.as_str() {
                        "initialize" => {
                            images_supported = result["agentCapabilities"]["promptCapabilities"]["image"] == true;
                            backend_version = result["agentInfo"]["version"].as_str().filter(|v| v.len() <= 64).map_or(Value::Null, |v| json!(v));
                            resumable = result["agentCapabilities"]["sessionCapabilities"].get("resume").is_some_and(Value::is_object) || result["agentCapabilities"]["loadSession"] == true;
                            if result["protocolVersion"] != 1 { return Err("OpenCode did not negotiate ACP v1".into()); }
                            let method = if let Some(id) = &session {
                                session_params["sessionId"] = json!(id);
                                if result["agentCapabilities"]["sessionCapabilities"].get("resume").is_some_and(Value::is_object) { "session/resume" }
                                else if result["agentCapabilities"]["loadSession"] == true { "session/load" }
                                else { return Err("OpenCode cannot restore this conversation".into()); }
                            } else { "session/new" };
                            next_id += 1; requests.insert(next_id,method.into()); rpc(input,next_id,method,session_params.clone()).await?;
                        }
                        "session/new" | "session/resume" | "session/load" => {
                            if method == "session/new" { session = Some(result["sessionId"].as_str().filter(|s| !s.is_empty()).ok_or("ACP session has no identity")?.into()); }
                            else if result.get("sessionId").is_some_and(|id| id.as_str() != session.as_deref()) { return Err("OpenCode restored a different session".into()); }
                            selected_model = model(result);
                            if let Some(model) = &setup.model {
                                next_id += 1; requests.insert(next_id,"session/set_config_option".into());
                                rpc(input,next_id,"session/set_config_option",json!({"sessionId":session,"configId":"model","value":model})).await?;
                            } else { ready = true; }
                        }
                        "session/set_config_option" => {
                            selected_model = model(result);
                            if selected_model.as_str() != setup.model.as_deref() { return Err("OpenCode did not confirm the requested model".into()); }
                            ready = true;
                        }
                        "session/prompt" => {
                            if active != Some(id) { return Err("ACP completed a different prompt".into()); }
                            let status = match result["stopReason"].as_str() { Some("end_turn") => "completed", Some("cancelled") => "interrupted", _ => "failed" };
                            active = None; stopping_at = None;
                            for (_, (id, _, _)) in permissions.drain() { emit(events,"requestResolved",json!({"requestId":id})).await?; }
                            emit(events,"terminal",json!({"status":status,"error":if status == "failed" { json!("OpenCode returned an unsuccessful or unknown stop reason") } else { Value::Null }})).await?;
                        }
                        _ => {}
                    }
                    if ready && ["session/new","session/resume","session/load","session/set_config_option"].contains(&method.as_str()) {
                        let mut ready = json!({"sessionId":session,"model":selected_model,"pid":pid,"backendVersion":backend_version,"resumable":resumable});
                        for (key, value) in process.as_object().into_iter().flatten() { ready[key] = value.clone(); }
                        emit(events,"ready",ready).await?;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if !ready && started.elapsed() > Duration::from_secs(60) { return Err("ACP initialization timed out".into()); }
                if stopping_at.is_some_and(|at: tokio::time::Instant| at.elapsed() > Duration::from_secs(10)) { return Err(super::backend::ACP_STOP_TIMEOUT.into()); }
            }
        }
        if ready && active.is_none() {
            if let Some(message) = queued.take() {
                tools.clear();
                turn_count += 1;
                text_item = format!("acp-{turn_count}-{}", message.id);
                next_id += 1;
                active = Some(next_id);
                requests.insert(next_id, "session/prompt".into());
                // ACP v1 has no per-session developer-instruction field. Keep
                // pinned-browser instructions in each prompt, outside page data.
                if !message.attachments.is_empty() && !images_supported {
                    return Err(
                        "This ACP agent does not advertise image input; the message was not sent"
                            .into(),
                    );
                }
                let mut prompt = vec![
                    json!({"type":"text","text":format!("{instructions}\n\n{}",message.prompt_text())}),
                ];
                for image in &message.attachments {
                    prompt.push(json!({"type":"image","mimeType":image.media_type,"data":image.base64(&setup.workspace)?}));
                }
                rpc(
                    input,
                    next_id,
                    "session/prompt",
                    json!({"sessionId":session,"prompt":prompt}),
                )
                .await?;
                emit(events, "running", json!({"turnId":next_id.to_string()})).await?;
            }
        }
    }
}

/// Remove CLI-owned history through the advertised API, never by guessing its
/// on-disk layout. Cleanup must be confirmed even if initialization fails.
pub(super) async fn delete_session(
    executable: &Path,
    workspace: &Path,
    session: &str,
) -> Result<(), String> {
    let mut probe = super::inspection::OwnedProbe::spawn(executable, &["acp"], workspace).await?;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut input = probe
            .child
            .stdin
            .take()
            .ok_or("ACP delete input unavailable")?;
        let mut reader = BufReader::new(
            probe
                .child
                .stdout
                .take()
                .ok_or("ACP delete output unavailable")?,
        );
        write(&mut input, initialize()).await?;
        let mut target = 1;
        let mut total = 0;
        loop {
            let mut bytes = Vec::new();
            let count = (&mut reader)
                .take(2 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut bytes)
                .await
                .map_err(|_| "Cannot read ACP deletion reply")?;
            total += count;
            if count == 0 || total > 2 * 1024 * 1024 || !bytes.ends_with(b"\n") {
                return Err("ACP deletion reply exceeded its bound or closed".into());
            }
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid ACP deletion reply")?;
            if value["id"] != target || value.get("method").is_some() {
                continue;
            }
            if value.get("error").is_some() || !value["result"].is_object() {
                return Err("OpenCode did not confirm session deletion".into());
            }
            if target == 2 {
                return Ok(());
            }
            if value["result"]["protocolVersion"] != 1
                || !value["result"]["agentCapabilities"]["sessionCapabilities"]
                    .get("delete")
                    .is_some_and(Value::is_object)
            {
                return Err("OpenCode does not advertise session deletion".into());
            }
            target = 2;
            rpc(
                &mut input,
                2,
                "session/delete",
                json!({"sessionId":session}),
            )
            .await?;
        }
    })
    .await
    .map_err(|_| "OpenCode session deletion timed out");
    probe.finish().await?;
    result?
}
