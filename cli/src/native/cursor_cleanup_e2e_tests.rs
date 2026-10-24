//! Real DOM acceptance for the extension's injected cursor expressions.
use super::*;

#[tokio::test]
#[ignore]
async fn e2e_cursor_cleanup_removes_real_dom_and_page_global() {
    let module = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../extensions/ab-connect/cursor-overlay.js");
    let driver = format!(
        "import {{pathToFileURL}} from 'node:url'; const {{cursorOverlayExpression, REMOVE_CURSOR_EXPRESSION}} = await import(pathToFileURL({})); console.log(JSON.stringify({{draw:cursorOverlayExpression(30,40,true),remove:REMOVE_CURSOR_EXPRESSION}}));",
        serde_json::to_string(&module.to_str().unwrap()).unwrap()
    );
    let output = std::process::Command::new("node")
        .args(["--input-type=module", "-e", &driver])
        .output()
        .expect("Node is required for cursor expression acceptance");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expressions: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut state = DaemonState::new();
    launch_headless(&mut state, json!({})).await;
    assert_success(
        &run_cmd(
            &mut state,
            json!({
                "id": "cursor-fixture", "action": "navigate",
                "url": "data:text/html,<title>cursor fixture</title><body>fixture</body>"
            }),
        )
        .await,
    );
    let before = eval_in(&mut state, "Object.getOwnPropertyNames(window).sort()").await;
    assert_eq!(
        eval_in(&mut state, expressions["draw"].as_str().unwrap()).await,
        true
    );
    assert_eq!(
        eval_in(
            &mut state,
            "window.__abCursor.host.isConnected && window.__abCursor.sr.children.length === 3"
        )
        .await,
        true
    );
    assert_eq!(
        eval_in(&mut state, expressions["remove"].as_str().unwrap()).await,
        true
    );
    assert_eq!(
        eval_in(&mut state, "Object.getOwnPropertyNames(window).sort()").await,
        before
    );
    assert_eq!(
        eval_in(
            &mut state,
            "document.querySelectorAll('body > div, html > div').length"
        )
        .await,
        0
    );
    // An animation frame and click timeout must not recreate an overlay later.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(eval_in(&mut state, "('__abCursor' in window) || document.querySelectorAll('body > div, html > div').length > 0").await, false);
    close_state(&mut state).await;
}
