//! Exercise the trusted contenteditable completion path through fake CDP.
use super::{finish_fill, CdpClient};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

async fn fill_with_readback(actual: &str) -> (Result<Option<String>, String>, Vec<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let actual = actual.to_string();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.into_text() else {
                continue;
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            log.lock().unwrap().push(request.clone());
            let result = match request["method"].as_str().unwrap() {
                "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"frame"}}}),
                "Page.createIsolatedWorld" => json!({"executionContextId":42}),
                "Input.insertText" => {
                    assert_eq!(request["params"]["text"], "hello world");
                    json!({})
                }
                "Runtime.evaluate" => json!({"result":{"value":true}}),
                "Runtime.callFunctionOn" => {
                    let script = request["params"]["functionDeclaration"].as_str().unwrap();
                    if script.contains("const s = '.monaco-editor") {
                        json!({"result":{"value":false}})
                    } else {
                        json!({"result":{"value":{"ok":true,"value":actual}}})
                    }
                }
                other => panic!("unexpected CDP method: {other}"),
            };
            if ws
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"id":request["id"],"result":result}).to_string(),
                ))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let client = CdpClient::connect(&format!("ws://{address}"))
        .await
        .unwrap();
    let mut engine = "contenteditable".to_string();
    let result = finish_fill(&client, "session", "editor", "hello world", &mut engine).await;
    server.abort();
    let commands = seen.lock().unwrap().clone();
    (result, commands)
}

fn assert_no_synthetic_edit(commands: &[Value]) {
    assert_eq!(
        commands
            .iter()
            .filter(|r| r["method"] == "Input.insertText")
            .count(),
        1
    );
    for request in commands {
        let script = request["params"]["functionDeclaration"]
            .as_str()
            .unwrap_or("");
        assert!(
            !script.contains("dispatchEvent"),
            "synthetic events: {request}"
        );
        assert!(
            !script.contains("execCommand"),
            "synthetic retry: {request}"
        );
        assert!(
            !script.contains("textContent ="),
            "DOM overwrite: {request}"
        );
        assert!(!script.contains(".blur("), "unexpected blur: {request}");
    }
}

#[tokio::test]
async fn contenteditable_success_verifies_without_synthetic_tail() {
    let (result, commands) = fill_with_readback("hello world").await;
    assert!(result.unwrap().is_none());
    assert_no_synthetic_edit(&commands);
    assert!(commands.iter().any(|r| r["params"]["functionDeclaration"]
        .as_str()
        .is_some_and(|s| s == super::read_editable_value_function())));
}

#[tokio::test]
async fn contenteditable_empty_read_fails_without_overwriting_editor_dom() {
    let (result, commands) = fill_with_readback("").await;
    assert!(result.unwrap_err().contains("read back an empty value"));
    assert_no_synthetic_edit(&commands);
}

#[tokio::test]
async fn contenteditable_formatting_keeps_its_existing_warning() {
    let (result, commands) = fill_with_readback("hello\u{a0}world\u{200b}").await;
    assert!(result
        .unwrap()
        .unwrap()
        .contains("editor formatted the text"));
    assert_no_synthetic_edit(&commands);
}
