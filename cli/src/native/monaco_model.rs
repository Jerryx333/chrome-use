//! Clipboard-free access to a Monaco API retained in handler module scopes.
//!
//! Discovery reads CDP descriptors, never invokes scope getters, and is bounded.
//! Only a public editor API matched to the exact DOM root may supply a model.
//! The model is read through the existing rich-editor main-world exception;
//! input remains native. No handles are stored on page objects or globals.
use std::collections::{HashSet, VecDeque};

use serde_json::{json, Value};

use super::cdp::client::CdpClient;

const MAX_OBJECTS: usize = 48;
const MAX_DEPTH: usize = 6;

#[cfg(test)]
#[path = "monaco_model_tests.rs"]
mod tests;

/// Read, or fill and verify, a hidden Monaco model without using the clipboard.
/// `None` means discovery found no usable public API before performing input.
/// Once a model is found, failures propagate rather than retrying via clipboard.
pub(crate) async fn access(
    client: &CdpClient,
    session: &str,
    element: &str,
    text: Option<&str>,
) -> Result<Option<String>, String> {
    let group = format!("monaco-{}", uuid::Uuid::new_v4().simple());
    let result = access_in_group(client, session, element, text, &group).await;
    // Release descriptor, scope and model handles on every ordinary exit,
    // including unsuccessful discovery and failures after input. A destroyed
    // context has already released them; it may reject this cleanup command.
    let _ = client
        .send_command(
            "Runtime.releaseObjectGroup",
            Some(json!({"objectGroup":group})),
            Some(session),
        )
        .await;
    result
}

async fn access_in_group(
    client: &CdpClient,
    session: &str,
    element: &str,
    text: Option<&str>,
    group: &str,
) -> Result<Option<String>, String> {
    let input = client.send_command("Runtime.callFunctionOn", Some(json!({
        "objectId":element,"objectGroup":group,
        "functionDeclaration":"function(){ const root=this.closest('.monaco-editor') || this.querySelector('.monaco-editor'); return root && root.querySelector('textarea.inputarea'); }"
    })), Some(session)).await?;
    check_exception(&input)?;
    let Some(input_id) = input.pointer("/result/objectId").and_then(Value::as_str) else {
        return Ok(None);
    };
    let listeners = client
        .send_command(
            "DOMDebugger.getEventListeners",
            Some(json!({"objectId":input_id})),
            Some(session),
        )
        .await?;
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    for listener in listeners["listeners"].as_array().into_iter().flatten() {
        if matches!(
            listener["type"].as_str(),
            Some("copy" | "input" | "keydown")
        ) {
            let handler = listener
                .get("originalHandler")
                .or_else(|| listener.get("handler"));
            if let Some(handler) = handler {
                enqueue(&mut queue, &mut seen, handler, 0, false, false);
            }
        }
    }
    for _ in 0..MAX_OBJECTS {
        let Some((object, depth, scope)) = queue.pop_front() else {
            break;
        };
        // Global bindings are already handled by the ordinary Monaco API path.
        // Do not scan unrelated page globals while exploring module handlers.
        if object["description"] == "Global" {
            continue;
        }
        let id = object["objectId"].as_str().unwrap();
        let properties = client
            .send_command(
                "Runtime.getProperties",
                Some(json!({"objectId":id,"ownProperties":true,"generatePreview":true})),
                Some(session),
            )
            .await?;
        check_exception(&properties)?;
        let descriptors = properties["result"].as_array().cloned().unwrap_or_default();
        if !scope && has_api_export(&descriptors, "getEditors") {
            let model = client
                .send_command(
                    "Runtime.callFunctionOn",
                    Some(json!({
                        "objectId":id,"objectGroup":group,"arguments":[{"objectId":input_id}],
                        "functionDeclaration":include_str!("monaco_model.js")
                    })),
                    Some(session),
                )
                .await?;
            check_exception(&model)?;
            if let Some(model_id) = model.pointer("/result/objectId").and_then(Value::as_str) {
                return operate(client, session, model_id, text).await.map(Some);
            }
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        for property in properties["internalProperties"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if matches!(
                property["name"].as_str(),
                Some("[[Scopes]]" | "[[TargetFunction]]" | "[[BoundThis]]")
            ) {
                enqueue(
                    &mut queue,
                    &mut seen,
                    &property["value"],
                    depth + 1,
                    false,
                    false,
                );
            }
        }
        for property in &descriptors {
            let name = property["name"].as_str().unwrap_or("");
            let value = &property["value"];
            let namespace = value
                .pointer("/preview/properties")
                .and_then(Value::as_array)
                .is_some_and(|properties| properties.iter().any(|p| p["name"] == "getEditors"));
            let numeric = name.parse::<usize>().is_ok();
            if namespace || numeric || matches!(name, "editor" | "monaco" | "api") {
                // V8's synthetic scope mirrors are not namespace objects and
                // cannot be called as one. Their descriptor values can be.
                let is_scope = numeric
                    && object["description"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("Scopes["));
                enqueue(&mut queue, &mut seen, value, depth + 1, is_scope, namespace);
            }
        }
    }
    Ok(None)
}

type Queue = VecDeque<(Value, usize, bool)>;

fn enqueue(
    queue: &mut Queue,
    seen: &mut HashSet<String>,
    value: &Value,
    depth: usize,
    scope: bool,
    first: bool,
) {
    if let Some(id) = value["objectId"].as_str() {
        if seen.len() < MAX_OBJECTS * 4 && seen.insert(id.to_owned()) {
            let item = (value.clone(), depth, scope);
            if first {
                queue.push_front(item);
            } else {
                queue.push_back(item);
            }
        }
    }
}

fn has_api_export(descriptors: &[Value], name: &str) -> bool {
    descriptors.iter().any(|p| {
        p["name"] == name
            && (p.pointer("/value/type").and_then(Value::as_str) == Some("function")
                || p.pointer("/get/type").and_then(Value::as_str) == Some("function"))
    })
}

fn check_exception(response: &Value) -> Result<(), String> {
    if let Some(exception) = response.get("exceptionDetails") {
        return Err(format!(
            "Monaco model access failed: {}",
            exception
                .pointer("/exception/description")
                .or_else(|| exception.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("page exception")
        ));
    }
    Ok(())
}

async fn call(
    client: &CdpClient,
    session: &str,
    model: &str,
    method: &str,
) -> Result<Value, String> {
    let response = client.send_command("Runtime.callFunctionOn", Some(json!({
        "objectId":model,"functionDeclaration":format!("function(){{ return this.{method}(); }}"),"returnByValue":true
    })), Some(session)).await?;
    check_exception(&response)?;
    response
        .pointer("/result/value")
        .cloned()
        .ok_or_else(|| "Monaco model returned no value".into())
}

async fn operate(
    client: &CdpClient,
    session: &str,
    model: &str,
    text: Option<&str>,
) -> Result<String, String> {
    if let Some(text) = text {
        call(client, session, model, "focus").await?;
        super::interaction::press_key_with_modifiers(
            client,
            session,
            "a",
            Some(super::interaction::platform_command_modifier()),
        )
        .await?;
        call(client, session, model, "checkFocus").await?;
        if text.is_empty() {
            super::interaction::press_key(client, session, "Backspace").await?;
        } else {
            client
                .send_command(
                    "Input.insertText",
                    Some(json!({"text":text})),
                    Some(session),
                )
                .await?;
        }
    }
    let result = call(client, session, model, "read").await?;
    let actual = result
        .as_str()
        .ok_or("Monaco model did not return text")?
        .to_owned();
    if let Some(expected) = text {
        if actual.replace("\r\n", "\n") != expected.replace("\r\n", "\n") {
            return Err(format!("Monaco native fill verification failed: read {} characters after writing {}; inspect the editor before retrying", actual.chars().count(), expected.chars().count()));
        }
    }
    Ok(actual)
}
