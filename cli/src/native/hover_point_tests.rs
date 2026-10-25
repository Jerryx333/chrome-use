//! Coordinate-hover lifecycle tests use a fake CDP server, never Chrome.
use super::{hover_point, CdpClient, HOVER_POINT_PROBE_JS};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    Input,
    PendingInput,
    Stop,
    PendingStop,
}

struct Fixture {
    client: Arc<CdpClient>,
    seen: Arc<Mutex<Vec<Value>>>,
    input: Arc<Notify>,
    stop: Arc<Notify>,
    released: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn observed(signal: &Notify) {
    tokio::time::timeout(Duration::from_secs(3), signal.notified())
        .await
        .expect("fake CDP did not observe the expected command");
}

async fn fixture(fault: Fault) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let input = Arc::new(Notify::new());
    let stop = Arc::new(Notify::new());
    let released = Arc::new(Notify::new());
    let (log, input_signal, stop_signal, release_signal) =
        (seen.clone(), input.clone(), stop.clone(), released.clone());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut held_stop = false;
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.into_text() else {
                continue;
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            log.lock().unwrap().push(request.clone());
            let method = request["method"].as_str().unwrap();
            let mut error = None;
            let result = match method {
                "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"frame"}}}),
                "Page.createIsolatedWorld" => json!({"executionContextId":42}),
                "Runtime.evaluate" => {
                    if request["params"]["expression"] == HOVER_POINT_PROBE_JS {
                        assert_eq!(request["params"]["contextId"], 42);
                        json!({"result":{"type":"object","objectId":"point-recorder"}})
                    } else {
                        json!({"result":{"type":"string","value":"visible"}})
                    }
                }
                "Input.dispatchMouseEvent" => {
                    input_signal.notify_one();
                    if fault == Fault::PendingInput {
                        continue;
                    }
                    if fault == Fault::Input {
                        error = Some("input rejected");
                    }
                    json!({})
                }
                "Runtime.callFunctionOn" => {
                    assert_eq!(request["params"]["objectId"], "point-recorder");
                    if request["params"]["functionDeclaration"]
                        .as_str()
                        .unwrap()
                        .contains("this.stop()")
                    {
                        stop_signal.notify_one();
                        if fault == Fault::PendingStop && !held_stop {
                            held_stop = true;
                            continue;
                        }
                        if fault == Fault::Stop {
                            error = Some("stop rejected");
                        }
                        json!({})
                    } else {
                        json!({"result":{"value":{"hit":"<button>","under":"<button>","frame":false}}})
                    }
                }
                "Runtime.releaseObject" => {
                    assert_eq!(request["params"]["objectId"], "point-recorder");
                    release_signal.notify_one();
                    json!({})
                }
                other => panic!("unexpected CDP method: {other}"),
            };
            let mut reply = json!({"id":request["id"],"result":result});
            if let Some(message) = error {
                reply = json!({"id":request["id"],"error":{"code":-32000,"message":message}});
            }
            if ws
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    reply.to_string(),
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
    Fixture {
        client,
        seen,
        input,
        stop,
        released,
        server,
    }
}

fn cleanup_methods(fixture: &Fixture) -> Vec<String> {
    fixture
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|request| {
            let method = request["method"].as_str()?;
            let stopping = request["params"]["functionDeclaration"]
                .as_str()
                .is_some_and(|f| f.contains("this.stop()"));
            (stopping || method == "Runtime.releaseObject").then(|| method.to_string())
        })
        .collect()
}

#[tokio::test]
async fn coordinate_hover_input_error_stops_and_releases_recorder() {
    let f = fixture(Fault::Input).await;
    let result = hover_point(&f.client, "session", 10.0, 20.0, None).await;
    assert!(result.unwrap_err().contains("input rejected"));
    observed(&f.released).await;
    assert_eq!(
        cleanup_methods(&f),
        ["Runtime.callFunctionOn", "Runtime.releaseObject"]
    );
}

#[tokio::test]
async fn coordinate_hover_normal_completion_cleans_up_once() {
    let f = fixture(Fault::None).await;
    assert!(
        hover_point(&f.client, "session", 10.0, 20.0, None)
            .await
            .unwrap()
            .verified
    );
    assert_eq!(
        cleanup_methods(&f),
        ["Runtime.callFunctionOn", "Runtime.releaseObject"]
    );
}

#[tokio::test]
async fn coordinate_hover_releases_handle_even_when_stop_is_rejected() {
    let f = fixture(Fault::Stop).await;
    hover_point(&f.client, "session", 10.0, 20.0, None)
        .await
        .unwrap();
    assert_eq!(
        cleanup_methods(&f),
        ["Runtime.callFunctionOn", "Runtime.releaseObject"]
    );
    // Listener expiry when stop cannot run is covered by the JS payload tests.
}

#[tokio::test]
async fn coordinate_hover_cancelled_during_input_cleans_up() {
    let f = fixture(Fault::PendingInput).await;
    let client = f.client.clone();
    let task = tokio::spawn(async move { hover_point(&client, "session", 10.0, 20.0, None).await });
    observed(&f.input).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    observed(&f.released).await;
    assert_eq!(
        cleanup_methods(&f),
        ["Runtime.callFunctionOn", "Runtime.releaseObject"]
    );
}

#[tokio::test]
async fn coordinate_hover_cancelled_during_stop_retries_cleanup() {
    let f = fixture(Fault::PendingStop).await;
    let client = f.client.clone();
    let task = tokio::spawn(async move { hover_point(&client, "session", 10.0, 20.0, None).await });
    observed(&f.stop).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    observed(&f.released).await;
    assert_eq!(
        cleanup_methods(&f),
        [
            "Runtime.callFunctionOn",
            "Runtime.callFunctionOn",
            "Runtime.releaseObject"
        ]
    );
}
