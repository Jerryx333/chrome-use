//! Browser acceptance for contenteditable event trust and editor-owned state.
use super::*;

#[tokio::test]
#[ignore]
async fn e2e_contenteditable_fill_preserves_focus_and_native_events() {
    let html = r##"<!doctype html><meta charset="utf-8">
      <div id="editor" contenteditable="true" role="textbox">old text</div>
      <output id="model"></output><output id="events">[]</output>
      <script>
      const editor = document.getElementById('editor');
      const model = document.getElementById('model');
      const events = document.getElementById('events');
      for (const type of ['beforeinput','input','change','blur','focusout']) {
        editor.addEventListener(type, event => {
          const log = JSON.parse(events.textContent);
          log.push({type: event.type, trusted: event.isTrusted});
          events.textContent = JSON.stringify(log);
          if (type === 'input') {
            if (editor.dataset.reject === 'true') editor.textContent = '';
            if (event.isTrusted) model.textContent = editor.textContent;
          }
        });
      }
      </script>"##;
    let (port, server) = spawn_html_server(html.to_string()).await;
    let mut state = DaemonState::new();
    launch_on(port, &mut state).await;
    let response = run_cmd(
        &mut state,
        json!({"id":"fill","action":"fill","selector":"#editor","value":"hello world"}),
    )
    .await;
    assert_success(&response);
    assert_eq!(
        eval_in(&mut state, "document.getElementById('model').textContent").await,
        "hello world"
    );
    assert_eq!(
        eval_in(&mut state, "document.activeElement.id").await,
        "editor"
    );
    let events = eval_in(
        &mut state,
        "JSON.parse(document.getElementById('events').textContent)",
    )
    .await;
    let events = events.as_array().unwrap();
    assert!(events.iter().any(|e| e["type"] == "input"));
    assert!(events.iter().all(|e| e["trusted"] == true));
    assert!(events
        .iter()
        .all(|e| e["type"] != "change" && e["type"] != "focusout" && e["type"] != "blur"));

    eval_in(&mut state, "document.getElementById('editor').dataset.reject='true';document.getElementById('events').textContent='[]'").await;
    let rejected = run_cmd(
        &mut state,
        json!({"id":"reject","action":"fill","selector":"#editor","value":"rejected"}),
    )
    .await;
    assert_eq!(rejected["success"], false, "{rejected}");
    assert_eq!(
        eval_in(&mut state, "document.getElementById('editor').textContent").await,
        ""
    );
    assert_eq!(eval_in(&mut state, "JSON.parse(document.getElementById('events').textContent).filter(e=>!e.trusted).length").await, 0);
    run_cmd(&mut state, json!({"id":"close","action":"close"})).await;
    server.abort();
}
