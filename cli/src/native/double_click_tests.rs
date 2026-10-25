//! Protocol tests exercise delivery without launching a browser.
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    Covered,
    MovesAfterClick,
    NoEvents,
}

async fn run(fault: Fault) -> (Result<DoubleClickOutcome, String>, Vec<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let released = Arc::new(tokio::sync::Notify::new());
    let release = released.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut clicks = 0;
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.into_text() else {
                continue;
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            log.lock().unwrap().push(request.clone());
            let result = match request["method"].as_str().unwrap() {
                "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"frame"}}}),
                "Page.createIsolatedWorld" => json!({"executionContextId":42}),
                "Runtime.evaluate" => json!({"result":{"type":"string","value":"visible"}}),
                "Input.dispatchMouseEvent" => {
                    assert_eq!(request["sessionId"], "top-session");
                    if request["params"]["type"] == "mouseReleased" {
                        clicks += 1;
                    }
                    json!({})
                }
                "Runtime.callFunctionOn" => {
                    let script = request["params"]["functionDeclaration"].as_str().unwrap();
                    assert!(
                        !script.contains("dispatchEvent"),
                        "synthetic input: {script}"
                    );
                    if script == RECORD {
                        json!({"result":{"type":"object","objectId":"recorder"}})
                    } else if script.contains("this.stop()") {
                        json!({"result":{"type":"undefined"}})
                    } else if script.contains("return this.events") {
                        let events = if fault == Fault::NoEvents {
                            json!([])
                        } else {
                            json!([{"type":"click","detail":1},{"type":"click","detail":2},{"type":"dblclick","detail":2}])
                        };
                        json!({"result":{"type":"object","value":events}})
                    } else {
                        let value = if fault == Fault::Covered {
                            json!({"error":"covered"})
                        } else if fault == Fault::MovesAfterClick && clicks > 0 {
                            json!({"points":[{"gx":100,"gy":200}]})
                        } else {
                            json!({"points":[{"gx":10,"gy":20}]})
                        };
                        json!({"result":{"type":"object","value":value}})
                    }
                }
                "Runtime.releaseObject" => {
                    release.notify_one();
                    json!({})
                }
                other => panic!("unexpected command: {other}"),
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
    let client = Arc::new(
        CdpClient::connect(&format!("ws://{address}"))
            .await
            .unwrap(),
    );
    let result = deliver(&client, "top-session", "element-session", "element").await;
    if fault != Fault::Covered {
        tokio::time::timeout(std::time::Duration::from_secs(3), released.notified())
            .await
            .unwrap();
    }
    server.abort();
    let commands = seen.lock().unwrap().clone();
    (result, commands)
}

fn pairs(commands: &[Value]) -> Vec<(String, i64, i64)> {
    commands
        .iter()
        .filter(|r| {
            r["method"] == "Input.dispatchMouseEvent" && r["params"]["type"] != "mouseMoved"
        })
        .map(|r| {
            (
                r["params"]["type"].as_str().unwrap().to_string(),
                r["params"]["clickCount"].as_i64().unwrap(),
                r["params"]["buttons"].as_i64().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn trusted_double_click_sends_two_complete_pairs_to_top_session() {
    let (result, commands) = run(Fault::None).await;
    assert!(result.unwrap().verified);
    assert_eq!(
        pairs(&commands),
        vec![
            ("mousePressed".into(), 1, 1),
            ("mouseReleased".into(), 1, 0),
            ("mousePressed".into(), 2, 1),
            ("mouseReleased".into(), 2, 0)
        ]
    );
}

#[tokio::test]
async fn covered_target_receives_no_input() {
    let (result, commands) = run(Fault::Covered).await;
    assert!(result.is_err());
    assert!(!commands
        .iter()
        .any(|r| r["method"] == "Input.dispatchMouseEvent"));
}

#[tokio::test]
async fn moving_target_stops_before_second_click_without_replay() {
    let (result, commands) = run(Fault::MovesAfterClick).await;
    assert!(result.err().unwrap().contains("before click 2"));
    assert_eq!(pairs(&commands).len(), 2);
}

#[tokio::test]
async fn missing_trusted_delivery_reports_uncertainty() {
    let (result, _) = run(Fault::NoEvents).await;
    let outcome = result.unwrap();
    assert!(!outcome.verified);
    assert!(outcome.warning.unwrap().contains("could not be confirmed"));
}
