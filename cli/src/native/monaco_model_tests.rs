//! Protocol controls for discovery, native input, refusal and handle cleanup.
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};

async fn exercise(mode: &str, value: Option<&str>) -> (Result<Option<String>, String>, Vec<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let mode = mode.to_owned();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.into_text() else {
                continue;
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            log.lock().unwrap().push(request.clone());
            let p = &request["params"];
            let result = match request["method"].as_str().unwrap() {
                "DOMDebugger.getEventListeners" if mode == "missing" => json!({"listeners":[]}),
                "DOMDebugger.getEventListeners" => {
                    json!({"listeners":[{"type":"copy","handler":{"type":"function","objectId":"handler"}}]})
                }
                "Runtime.getProperties" => match p["objectId"].as_str().unwrap() {
                    "handler" => {
                        json!({"internalProperties":[{"name":"[[Scopes]]","value":{"objectId":"scopes","description":"Scopes[1]"}}]})
                    }
                    "scopes" => {
                        json!({"result":[{"name":"0","value":{"objectId":"module","description":"Module"}}]})
                    }
                    // The synthetic mirror advertises getEditors, but only the
                    // real API binding may be used as a callable namespace.
                    "module" => {
                        json!({"result":[{"name":"getEditors","value":{"type":"function"}},{"name":"minified_name","value":{"objectId":"api","preview":{"properties":[{"name":"getEditors","type":"accessor"}]}}}]})
                    }
                    "api" => json!({"result":[{"name":"getEditors","get":{"type":"function"}}]}),
                    other => panic!("unexpected discovery object {other}"),
                },
                "Runtime.callFunctionOn" => match p["objectId"].as_str().unwrap() {
                    "element" => json!({"result":{"objectId":"input"}}),
                    "api" => json!({"result":{"objectId":"model"}}),
                    "model" => {
                        let script = p["functionDeclaration"].as_str().unwrap();
                        if script.contains("checkFocus") && mode == "focus-lost" {
                            json!({"exceptionDetails":{"text":"editor lost focus"}})
                        } else if script.contains("read()") {
                            json!({"result":{"value":if mode=="mismatch" {"different"} else {"expected"}}})
                        } else {
                            json!({"result":{"value":true}})
                        }
                    }
                    other => panic!("must not invoke scope mirrors: {other}"),
                },
                "Input.dispatchKeyEvent" | "Input.insertText" | "Runtime.releaseObjectGroup" => {
                    json!({})
                }
                other => panic!("unexpected command {other}"),
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
    let result = access(&client, "session", "element", value).await;
    let commands = seen.lock().unwrap().clone();
    server.abort();
    (result, commands)
}

fn assert_cleanup(commands: &[Value]) {
    assert_eq!(
        commands.last().unwrap()["method"],
        "Runtime.releaseObjectGroup"
    );
    assert_eq!(
        commands.last().unwrap()["params"]["objectGroup"],
        commands[0]["params"]["objectGroup"]
    );
}

#[tokio::test]
async fn native_fill_uses_exact_model_and_releases_handles() {
    let (result, commands) = exercise("ok", Some("expected")).await;
    assert_eq!(result.unwrap().as_deref(), Some("expected"));
    let input: Vec<_> = commands
        .iter()
        .filter(|c| c["method"].as_str().unwrap().starts_with("Input."))
        .collect();
    assert_eq!(input.len(), 3);
    assert_eq!(input[0]["params"]["commands"], json!(["selectAll"]));
    assert_eq!(input[2]["method"], "Input.insertText");
    assert_eq!(input[2]["params"]["text"], "expected");
    assert_cleanup(&commands);
}

#[tokio::test]
async fn model_read_and_unavailable_discovery_perform_no_input() {
    for mode in ["ok", "missing"] {
        let (result, commands) = exercise(mode, None).await;
        assert_eq!(
            result.unwrap().as_deref(),
            if mode == "ok" { Some("expected") } else { None }
        );
        assert!(commands
            .iter()
            .all(|c| !c["method"].as_str().unwrap().starts_with("Input.")));
        assert_cleanup(&commands);
    }
}

#[tokio::test]
async fn focus_loss_refuses_insert_and_mismatch_does_not_retry() {
    for mode in ["focus-lost", "mismatch"] {
        let (result, commands) = exercise(mode, Some("expected")).await;
        assert!(result.is_err());
        let inserts = commands
            .iter()
            .filter(|c| c["method"] == "Input.insertText")
            .count();
        assert_eq!(inserts, usize::from(mode == "mismatch"));
        assert_cleanup(&commands);
    }
}
