//! Protocol fault injection. Payload semantics are checked separately by Node;
//! these fixtures do not establish real Chrome clipboard permissions or events.
use super::{fill_monaco_via_clipboard, CdpClient};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

async fn run(
    setup_error: bool,
    input_error: bool,
    cleanup_error: bool,
) -> (Result<String, String>, Vec<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.into_text() else {
                continue;
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            log.lock().unwrap().push(request.clone());
            let mut error = None;
            let result = match request["method"].as_str().unwrap() {
                "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"frame"}}}),
                "Page.createIsolatedWorld" => json!({"executionContextId":42}),
                "Runtime.evaluate" => json!({"result":{"type":"boolean","value":true}}),
                "Runtime.releaseObject" => json!({}),
                "Input.dispatchKeyEvent" => {
                    if input_error {
                        error = Some(json!({"code":-32000,"message":"input rejected"}));
                    }
                    json!({})
                }
                "Runtime.callFunctionOn" => {
                    let script = request["params"]["functionDeclaration"].as_str().unwrap();
                    if script == include_str!("monaco_clipboard.js") {
                        assert_eq!(request["params"]["returnByValue"], false);
                        json!({"result":{"type":"object","objectId":"transaction"}})
                    } else {
                        assert_eq!(request["params"]["objectId"], "transaction");
                        let data = if script.contains("this.start(") {
                            if setup_error {
                                json!({"ok":false,"error":"focus failed"})
                            } else {
                                json!({"ok":true})
                            }
                        } else if script.contains("this.active()") {
                            json!({"ok":true})
                        } else if script.contains("this.finish()") {
                            if cleanup_error {
                                json!({"ok":false,"error":"restore denied"})
                            } else {
                                json!({"ok":true})
                            }
                        } else {
                            json!({"trusted":true,"prevented":true,"text":"replacement"})
                        };
                        json!({"result":{"type":"object","value":data}})
                    }
                }
                other => panic!("unexpected CDP method: {other}"),
            };
            let response = match error {
                Some(error) => json!({"id":request["id"],"error":error}),
                None => json!({"id":request["id"],"result":result}),
            };
            if ws
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    response.to_string(),
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
    let result = fill_monaco_via_clipboard(&client, "session", "anchor", "replacement").await;
    server.abort();
    let commands = seen.lock().unwrap().clone();
    (result, commands)
}
fn assert_finished(commands: &[Value]) {
    assert_eq!(commands.iter().filter(|r| r["params"]["functionDeclaration"] == "function() { return this.finish(); }").count(), 1);
    assert_eq!(commands.last().unwrap()["method"], "Runtime.releaseObject");
}
#[tokio::test]
async fn monaco_private_handle_success_is_verified_and_released() {
    let (result, commands) = run(false, false, false).await;
    assert_eq!(result.unwrap(), "monaco-clipboard");
    assert_finished(&commands);
}
#[tokio::test]
async fn monaco_setup_failure_still_finishes_and_never_sends_input() {
    let (result, commands) = run(true, false, false).await;
    assert!(result.unwrap_err().contains("focus failed"));
    assert!(!commands
        .iter()
        .any(|r| r["method"] == "Input.dispatchKeyEvent"));
    assert_finished(&commands);
}
#[tokio::test]
async fn monaco_input_failure_keeps_both_operation_and_cleanup_errors() {
    let (result, commands) = run(false, true, true).await;
    let error = result.unwrap_err();
    assert!(error.contains("input rejected"));
    assert!(error.contains("restore denied"));
    assert_finished(&commands);
}
#[tokio::test]
async fn monaco_cleanup_failure_is_not_reported_as_fill_success() {
    let (result, commands) = run(false, false, true).await;
    assert!(result.unwrap_err().contains("restore denied"));
    assert_finished(&commands);
}
