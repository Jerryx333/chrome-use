//! Fail closed at the transport boundary; a fake server records every request.
use super::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

async fn scenario(mode: &str, method: &str, params: Value) -> (Result<Value, String>, Vec<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let mode = mode.to_string();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut worlds = 0;
        let mut operations = 0;
        while let Some(Ok(Message::Text(raw))) = ws.next().await {
            let request: Value = serde_json::from_str(&raw).unwrap();
            log.lock().unwrap().push(request.clone());
            let mut error = None;
            let result = match request["method"].as_str().unwrap() {
                "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"main"}}}),
                "Page.createIsolatedWorld" => {
                    worlds += 1;
                    if mode == "creation" || (mode == "recreation" && worlds > 1) {
                        error = Some("No frame for given id");
                    }
                    json!({"executionContextId":worlds + 40})
                }
                "Runtime.evaluate" | "DOM.resolveNode" => {
                    operations += 1;
                    let p = &request["params"];
                    if p.get("contextId").is_some() || p.get("executionContextId").is_some() {
                        if mode == "recreation" || (mode == "frame-after-retry" && operations == 1)
                        {
                            error = Some("Cannot find context with specified id");
                        } else if mode == "frame" || mode == "frame-after-retry" {
                            error = Some("Node does not belong to the execution context");
                        }
                    }
                    json!({"result":{"type":"boolean","value":true},"object":{"type":"object","objectId":"node"}})
                }
                other => panic!("unexpected CDP method: {other}"),
            };
            let reply = match error {
                Some(message) => {
                    json!({"id":request["id"],"error":{"code":-32000,"message":message}})
                }
                None => json!({"id":request["id"],"result":result}),
            };
            if ws.send(Message::Text(reply.to_string())).await.is_err() {
                break;
            }
        }
    });
    let client = CdpClient::connect(&format!("ws://{address}"))
        .await
        .unwrap();
    let result = client
        .send_command(method, Some(params), Some("session"))
        .await;
    server.abort();
    let commands = seen.lock().unwrap().clone();
    (result, commands)
}
#[tokio::test]
async fn required_isolation_never_dispatches_without_context_after_failure() {
    for mode in ["creation", "recreation", "frame", "frame-after-retry"] {
        for method in ["Runtime.evaluate", "DOM.resolveNode"] {
            if method == "Runtime.evaluate" && mode.starts_with("frame") {
                continue;
            }
            let (result, seen) =
                scenario(mode, method, json!({"expression":"1","backendNodeId":1})).await;
            assert!(
                result.unwrap_err().contains("isolation_unavailable"),
                "{mode}: {method}"
            );
            for request in seen.iter().filter(|r| r["method"] == method) {
                let key = if method == "Runtime.evaluate" {
                    "contextId"
                } else {
                    "executionContextId"
                };
                assert!(
                    request["params"].get(key).is_some(),
                    "main-world fallback: {request}"
                );
            }
            if mode == "creation" {
                assert!(!seen.iter().any(|r| r["method"] == method));
            }
        }
    }
}
#[tokio::test]
async fn intentional_main_world_and_explicit_contexts_keep_their_contract() {
    for (method, params) in [
        (
            "Runtime.evaluate",
            json!({"expression":"1","__cuMain":true}),
        ),
        (
            "DOM.resolveNode",
            json!({"backendNodeId":1,"__cuMain":true}),
        ),
        ("Runtime.evaluate", json!({"expression":"1","contextId":9})),
        (
            "Runtime.evaluate",
            json!({"expression":"1","uniqueContextId":"explicit"}),
        ),
        (
            "DOM.resolveNode",
            json!({"backendNodeId":1,"executionContextId":9}),
        ),
        (
            "Runtime.evaluate",
            json!({"expression":"1","replMode":true}),
        ),
    ] {
        let (result, seen) = scenario("creation", method, params).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0]["method"], method);
        assert!(seen[0]["params"].get("__cuMain").is_none());
    }
}
