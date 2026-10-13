//! Explicit, bounded CLI metadata probes. Discovery itself never runs a CLI.
//! The panel sends no prompts or tools and opens no credential files. ACP
//! metadata uses a transient session that is deleted before reporting success.
//! Authentication lookup remains inside the selected CLI.

use super::{config_transaction::private_tree, DiscoveryContext};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::Semaphore,
};

static PROBES: Semaphore = Semaphore::const_new(2);
const MAX_OUTPUT: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Inspect {
    agent_id: String,
    executable: Option<PathBuf>,
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) struct OwnedProbe {
    pub(super) child: Child,
    pid: u32,
}
impl OwnedProbe {
    pub(super) fn spawn(
        executable: &Path,
        args: &[&str],
        workspace: &Path,
    ) -> Result<Self, String> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .current_dir(workspace)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env_remove("CLAUDE_CODE_SSE_PORT");
        if let Some(path) = super::agent_search_path() {
            command.env("PATH", path);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        let child = command
            .spawn()
            .map_err(|_| "Cannot start the selected CLI metadata probe")?;
        let pid = child.id().ok_or("CLI probe has no process identity")?;
        Ok(Self { child, pid })
    }
    pub(super) async fn finish(&mut self) -> Result<std::process::ExitStatus, String> {
        if self
            .child
            .try_wait()
            .map_err(|_| "Cannot inspect CLI probe exit")?
            .is_none()
        {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
            }
            self.child
                .start_kill()
                .map_err(|_| "CLI probe termination is unconfirmed")?;
        }
        let status = tokio::time::timeout(Duration::from_secs(1), self.child.wait())
            .await
            .map_err(|_| "CLI probe exit is unconfirmed")?
            .map_err(|_| "CLI probe exit is unconfirmed")?;
        #[cfg(unix)]
        {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
            loop {
                if unsafe { libc::kill(-(self.pid as i32), 0) } < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err("CLI probe process group exit is unconfirmed".into());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        Ok(status)
    }
}
impl Drop for OwnedProbe {
    fn drop(&mut self) {
        // On RPC cancellation, kill only a still-live owned group before PID
        // reap. An already-exited leader never authorizes a PID-based kill.
        if matches!(self.child.try_wait(), Ok(None)) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
            }
            let _ = self.child.start_kill();
        }
    }
}

fn version(raw: &str) -> Option<String> {
    let text = raw.trim();
    let text = text.strip_prefix("opencode v").unwrap_or(text);
    let text = text.strip_prefix("codex-cli ").unwrap_or(text);
    let text = text.strip_suffix(" (Claude Code)").unwrap_or(text);
    if text.len() > 64
        || text.split('.').count() != 3
        || !text
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(text.into())
}
fn model_id(raw: &Value) -> Result<String, String> {
    let id = raw
        .as_str()
        .ok_or("CLI model entry has no string identifier")?;
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/()-[]".contains(&b))
        || id.starts_with("sk-")
        || id.starts_with("eyJ")
    {
        return Err("CLI model identifier has an unsupported shape".into());
    }
    Ok(id.into())
}
fn models(raw: &Value, codex: bool) -> Result<Vec<Value>, String> {
    let entries = raw.as_array().ok_or("CLI did not return a model array")?;
    if entries.len() > 256 {
        return Err("CLI returned too many models in one page".into());
    }
    entries.iter().filter(|entry|entry["hidden"]!=true).map(|entry| {
        let id=model_id(if codex {entry.get("model").or_else(||entry.get("id")).unwrap_or(&Value::Null)} else {&entry["value"]})?;
        let label=entry["displayName"].as_str().filter(|s|s.chars().count()<=128 && !s.chars().any(char::is_control)).unwrap_or(&id);
        let raw_efforts=if codex {&entry["supportedReasoningEfforts"]} else {&entry["supportedEffortLevels"]};
        let efforts:Vec<&str>=raw_efforts.as_array().into_iter().flatten().filter_map(|value|if codex {value["reasoningEffort"].as_str()}else{value.as_str()}).filter(|value|["minimal","low","medium","high","xhigh","max"].contains(value)).collect();
        Ok(json!({"id":id,"label":label,"efforts":efforts,"isDefault":entry["isDefault"]==true,"supportsFastMode":entry["supportsFastMode"]==true}))
    }).collect()
}
fn codex_account(raw: &Value) -> Value {
    match raw["account"]["type"].as_str() {
        Some("chatgpt" | "apiKey" | "chatgptAuthTokens") => {
            json!({"status":"reportedSignedIn","kind":raw["account"]["type"]})
        }
        _ if raw["account"].is_null() && raw["requiresOpenaiAuth"] == true => {
            json!({"status":"reportedSignedOut"})
        }
        _ => json!({"status":"unverified"}),
    }
}
async fn write(input: &mut tokio::process::ChildStdin, value: Value) -> Result<(), String> {
    let mut bytes = value.to_string().into_bytes();
    bytes.push(b'\n');
    input
        .write_all(&bytes)
        .await
        .map_err(|_| "CLI metadata input closed".into())
}
async fn read(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    total: &mut usize,
) -> Result<Value, String> {
    let mut bytes = Vec::new();
    let limit = MAX_OUTPUT.saturating_sub(*total);
    let count = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|_| "Cannot read CLI metadata")?;
    *total += count;
    if count == 0 {
        return Err("CLI metadata output closed before confirmation".into());
    }
    if *total > MAX_OUTPUT || bytes.last() != Some(&b'\n') {
        return Err("CLI metadata exceeded its bounded output".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "CLI metadata was not a JSON protocol reply".into())
}
/// Pi's RPC mode answers `get_available_models` with full model records; the
/// menu value is `provider/id`, which `pi --model` accepts. Pi keeps provider
/// credentials per provider, so no account state is claimed.
async fn pi_catalog(probe: &mut OwnedProbe) -> Result<Value, String> {
    let mut input = probe
        .child
        .stdin
        .take()
        .ok_or("CLI metadata input unavailable")?;
    let mut reader = BufReader::new(
        probe
            .child
            .stdout
            .take()
            .ok_or("CLI metadata output unavailable")?,
    );
    let mut total = 0;
    write(
        &mut input,
        json!({"id":"panel-models","type":"get_available_models"}),
    )
    .await?;
    for _ in 0..200 {
        let value = read(&mut reader, &mut total).await?;
        if value["type"] != "response" || value["id"] != "panel-models" {
            continue;
        }
        if value["success"] != true {
            return Err("Pi refused the model catalog request".into());
        }
        let entries = value["data"]["models"]
            .as_array()
            .ok_or("Pi did not return a model array")?;
        // Pi lists every configured model; the menu keeps the first 256 and
        // says the catalog is partial.
        let complete = entries.len() <= 256;
        let models = entries
            .iter()
            .take(256)
            .map(|entry| {
                let provider = entry["provider"].as_str().unwrap_or("");
                let raw = entry["id"].as_str().unwrap_or("");
                let id = model_id(&json!(if provider.is_empty() {
                    raw.to_string()
                } else {
                    format!("{provider}/{raw}")
                }))?;
                let label = entry["name"]
                    .as_str()
                    .filter(|s| s.chars().count() <= 128 && !s.chars().any(char::is_control))
                    .map(|name| if provider.is_empty() { name.to_string() } else { format!("{provider}/{name}") })
                    .unwrap_or_else(|| id.clone());
                let efforts: Vec<&str> = if entry["reasoning"] == true {
                    vec!["low", "medium", "high"]
                } else {
                    vec![]
                };
                Ok(json!({"id":id,"label":label,"efforts":efforts,"isDefault":false,"supportsFastMode":false}))
            })
            .collect::<Result<Vec<_>, String>>()?;
        drop(input);
        return Ok(json!({"models":models,"complete":complete,"account":{"status":"unverified"}}));
    }
    Err("Pi metadata reply limit exceeded".into())
}

async fn protocol(probe: &mut OwnedProbe, codex: bool) -> Result<Value, String> {
    let mut input = probe
        .child
        .stdin
        .take()
        .ok_or("CLI probe input unavailable")?;
    let mut reader = BufReader::new(
        probe
            .child
            .stdout
            .take()
            .ok_or("CLI probe output unavailable")?,
    );
    let mut total = 0;
    let mut entries = Vec::new();
    let mut account = json!({"status":"unverified"});
    if !codex {
        write(&mut input,json!({"type":"control_request","request_id":"panel-inspect-init","request":{"subtype":"initialize","hooks":null}})).await?;
        for _ in 0..64 {
            let reply = read(&mut reader, &mut total).await?;
            if reply["type"] != "control_response"
                || reply["response"]["request_id"] != "panel-inspect-init"
            {
                continue;
            }
            if reply["response"]["subtype"] != "success" {
                return Err("Claude rejected metadata initialization".into());
            }
            let body = &reply["response"]["response"];
            let catalog = models(&body["models"], false)?;
            if body["account"].as_object().is_some_and(|o| !o.is_empty()) {
                account = json!({"status":"reportedMetadata"});
            }
            return Ok(json!({"models":catalog,"complete":true,"account":account}));
        }
        return Err("Claude metadata reply limit exceeded".into());
    }
    write(&mut input,json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"chrome_use_inspection","version":env!("CARGO_PKG_VERSION")}}})).await?;
    let mut initialized = false;
    let mut account_done = false;
    let mut catalog_done = false;
    let mut complete = true;
    let mut page = 0u64;
    let mut cursors = HashSet::new();
    for _ in 0..128 {
        let reply = read(&mut reader, &mut total).await?;
        match reply["id"].as_u64() {
            Some(1) if !initialized => {
                if reply.get("error").is_some() || !reply["result"].is_object() {
                    return Err("Codex rejected metadata initialization".into());
                }
                initialized = true;
                write(&mut input, json!({"method":"initialized"})).await?;
                write(
                    &mut input,
                    json!({"id":2,"method":"account/read","params":{"refreshToken":false}}),
                )
                .await?;
                write(&mut input,json!({"id":3,"method":"model/list","params":{"limit":64,"includeHidden":false}})).await?;
            }
            Some(2) if initialized => {
                account_done = true;
                if reply.get("error").is_none() {
                    account = codex_account(&reply["result"]);
                }
            }
            Some(id) if initialized && !catalog_done && id == 3 + page => {
                if reply.get("error").is_some() {
                    return Err(
                        "Codex model listing failed; no fallback catalog was substituted".into(),
                    );
                }
                entries.extend(models(&reply["result"]["data"], true)?);
                if entries.len() > 256 {
                    return Err("Codex catalog exceeded the bounded model count".into());
                }
                if let Some(cursor) = reply["result"]["nextCursor"].as_str() {
                    if cursor.len() > 4096 {
                        return Err("Codex model-list cursor exceeded its bound".into());
                    }
                    if !cursors.insert(cursor.to_string()) {
                        return Err("Codex repeated a model-list cursor".into());
                    }
                    page += 1;
                    if page >= 4 {
                        complete = false;
                        catalog_done = true;
                    } else {
                        write(&mut input,json!({"id":3+page,"method":"model/list","params":{"limit":64,"includeHidden":false,"cursor":cursor}})).await?;
                    }
                } else if reply["result"]["nextCursor"].is_null() {
                    catalog_done = true;
                } else {
                    return Err("Codex model-list cursor is invalid".into());
                }
            }
            _ => {}
        }
        if catalog_done && account_done {
            return Ok(json!({"models":entries,"complete":complete,"account":account}));
        }
    }
    Err("Codex metadata reply limit exceeded".into())
}

async fn opencode_catalog(probe: &mut OwnedProbe, workspace: &Path) -> Result<Vec<Value>, String> {
    let mut input = probe
        .child
        .stdin
        .take()
        .ok_or("OpenCode metadata input unavailable")?;
    let mut reader = BufReader::new(
        probe
            .child
            .stdout
            .take()
            .ok_or("OpenCode metadata output unavailable")?,
    );
    let mut total = 0;
    async fn reply(
        reader: &mut BufReader<tokio::process::ChildStdout>,
        total: &mut usize,
        id: u64,
    ) -> Result<Value, String> {
        for _ in 0..128 {
            let value = read(reader, total).await?;
            if value["id"] == id && value.get("method").is_none() {
                if value.get("error").is_some() || !value["result"].is_object() {
                    return Err("OpenCode refused metadata inspection".into());
                }
                return Ok(value["result"].clone());
            }
        }
        Err("OpenCode metadata reply limit exceeded".into())
    }
    write(&mut input,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}})).await?;
    let capabilities = reply(&mut reader, &mut total, 1).await?;
    if capabilities["protocolVersion"] != 1
        || !capabilities["agentCapabilities"]["sessionCapabilities"]
            .get("delete")
            .is_some_and(Value::is_object)
    {
        return Err("OpenCode metadata requires ACP v1 and session deletion support".into());
    }
    write(&mut input,json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":workspace,"mcpServers":[]}})).await?;
    let result = reply(&mut reader, &mut total, 2).await?;
    let session = result["sessionId"]
        .as_str()
        .ok_or("OpenCode metadata session has no identity")?;
    // Delete even when the returned catalog is malformed. No prompts or tools
    // are sent, and the transient inspection session must not become history.
    write(
        &mut input,
        json!({"jsonrpc":"2.0","id":3,"method":"session/delete","params":{"sessionId":session}}),
    )
    .await?;
    reply(&mut reader, &mut total, 3).await?;
    let option = result["configOptions"]
        .as_array()
        .and_then(|options| {
            options
                .iter()
                .find(|o| o["id"] == "model" && o["type"] == "select")
        })
        .ok_or("OpenCode has no selectable model catalog")?;
    let entries = option["options"]
        .as_array()
        .ok_or("OpenCode model options are not an array")?;
    if entries.is_empty() || entries.len() > 256 {
        return Err("OpenCode returned an empty or oversized catalog".into());
    }
    entries.iter().map(|entry| {
        let id = model_id(&entry["value"])?;
        let label = entry["name"].as_str().filter(|s| s.chars().count() <= 128 && !s.chars().any(char::is_control)).unwrap_or(&id);
        Ok(json!({"id":id,"label":label,"efforts":[],"isDefault":entry["value"] == option["currentValue"]}))
    }).collect()
}

pub(super) async fn inspect(params: &Value, root: &Path) -> Result<Value, String> {
    #[cfg(not(unix))]
    {
        let _ = (params, root);
        return Err(
            "CLI metadata inspection requires owned process-group support on this platform".into(),
        );
    }
    #[cfg(unix)]
    {
        let _permit = PROBES
            .try_acquire()
            .map_err(|_| "Two CLI inspections are already running")?;
        let request: Inspect = serde_json::from_value(params.clone())
            .map_err(|_| "Inspection requires agentId and optional executable")?;
        if !["codex", "claude", "opencode", "pi"].contains(&request.agent_id.as_str()) {
            return Err("This agent has no metadata protocol adapter yet".into());
        }
        let context = DiscoveryContext::current()?;
        let executable = request
            .executable
            .or_else(|| context.executable(&request.agent_id))
            .ok_or("The selected agent CLI was not found")?;
        if !executable.is_absolute() {
            return Err("A custom agent executable must be an absolute path".into());
        }
        if !super::is_executable(&executable, false) {
            return Err("The selected CLI is not executable".into());
        }
        let workspace = root.join(format!("inspection-{}", uuid::Uuid::new_v4()));
        private_tree(&workspace)?;
        let _scratch = Scratch(workspace.clone());
        let result=async {
            let mut probe=OwnedProbe::spawn(&executable,&["--version"],&workspace)?;
            let mut output=probe.child.stdout.take().ok_or("CLI version output unavailable")?;
            let version_result=tokio::time::timeout(Duration::from_secs(2),async {let mut bytes=Vec::new();(&mut output).take(1025).read_to_end(&mut bytes).await.ok()?;version(std::str::from_utf8(&bytes).ok()?)}).await.ok().flatten();
            let version_exit=probe.finish().await?;
            let version_result=if version_exit.success() {version_result} else {None};
            if request.agent_id == "opencode" {
                let mut probe = OwnedProbe::spawn(&executable, &["acp"], &workspace)?;
                let catalog = tokio::time::timeout(Duration::from_secs(15), opencode_catalog(&mut probe, &workspace)).await.map_err(|_| "OpenCode model inspection timed out");
                probe.finish().await?;
                let catalog = catalog??;
                return Ok(json!({"schemaVersion":1,"inspection":{"agentId":request.agent_id,"executable":executable,"versionReported":version_result,"source":"cliProtocol","observedAt":super::conversation::now(),"models":catalog,"complete":true,"account":{"status":"unverified"},"modelAccessVerified":false,"processExitConfirmed":true}}));
            }
            let codex=request.agent_id=="codex";
            let pi=request.agent_id=="pi";
            let args=if pi {vec!["--mode","rpc","--no-session","--no-mcp","--no-tools","--no-approve"]} else if codex {vec!["app-server","--listen","stdio://"]} else {vec!["-p","--input-format","stream-json","--output-format","stream-json","--verbose","--strict-mcp-config","--mcp-config","{\"mcpServers\":{}}","--tools","","--no-session-persistence","--setting-sources","user","--settings","{\"disableAllHooks\":true}"]};
            let mut probe=OwnedProbe::spawn(&executable,&args,&workspace)?;
            let catalog=match tokio::time::timeout(Duration::from_secs(if pi {10} else {4}),async { if pi {pi_catalog(&mut probe).await} else {protocol(&mut probe,codex).await} }).await {Ok(result)=>result,Err(_)=>Err("CLI metadata inspection timed out".into())};
            let cleanup=probe.finish().await;
            let _=cleanup?;let catalog=catalog?;
            Ok(json!({"schemaVersion":1,"inspection":{"agentId":request.agent_id,"executable":executable,"versionReported":version_result,"source":"cliProtocol","observedAt":super::conversation::now(),"models":catalog["models"],"complete":catalog["complete"],"account":catalog["account"],"modelAccessVerified":false,"processExitConfirmed":true}}))
        }.await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_and_models_never_forward_credentials_or_identity_fields() {
        let account = codex_account(
            &json!({"account":{"type":"chatgpt","email":"private@example.invalid","accessToken":"synthetic-secret"},"requiresOpenaiAuth":true}),
        );
        assert_eq!(
            account,
            json!({"status":"reportedSignedIn","kind":"chatgpt"})
        );
        let entries=models(&json!([{"value":"sonnet","displayName":"Sonnet","supportedEffortLevels":["low","high"],"secret":"synthetic-secret"}]),false).unwrap();
        assert!(!serde_json::to_string(&entries)
            .unwrap()
            .contains("synthetic-secret"));
        assert_eq!(entries[0]["efforts"], json!(["low", "high"]));
        assert!(models(&json!([{"value":"sonnet[1m]"}]), false).is_ok());
    }
    #[test]
    fn malformed_catalogs_and_secret_like_ids_are_not_default_model_lists() {
        assert!(models(&json!({"models":[]}), true).is_err());
        assert!(models(&json!([{"model":"sk-synthetic-secret"}]), true).is_err());
        assert_eq!(version("codex-cli 0.162.1"), Some("0.162.1".into()));
        assert_eq!(version("2.1.296 (Claude Code)"), Some("2.1.296".into()));
        assert!(version("0.162.1 token=synthetic-secret").is_none());
    }
}
