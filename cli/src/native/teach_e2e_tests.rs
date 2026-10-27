//! `teach start` → trusted input → `teach stop` → compile → replay in a
//! fresh browser, on a local fixture.
use super::*;

const TEACH_FIXTURE: &str = r##"<!doctype html><title>Order</title>
<form id="f" action="/done" method="get">
  <label>Name <input id="name" name="name"></label>
  <label>Password <input id="pw" name="pw" type="password"></label>
  <label><input id="gift" name="gift" type="checkbox" value="yes"> Gift wrap</label>
  <button id="go" type="submit">Place order</button>
</form>
<p id="out"></p>
<script>
  const q = new URLSearchParams(location.search);
  if (location.pathname === '/done') {
    document.getElementById('f').hidden = true;
    document.getElementById('out').textContent =
      `ordered by ${q.get('name')} pw=${(q.get('pw') || '').length} gift=${q.get('gift') || 'no'}`;
  }
</script>"##;

#[tokio::test]
#[ignore]
async fn e2e_teach_records_trusted_input_and_the_script_replays_in_a_fresh_browser() {
    let (port, server) = spawn_html_server(TEACH_FIXTURE.to_string()).await;
    let mut rec = DaemonState::new();
    launch_on(port, &mut rec).await;

    let start = run_cmd(
        &mut rec,
        json!({"id": "t1", "action": "teach_start", "name": "order"}),
    )
    .await;
    assert_success(&start);
    assert_eq!(get_data(&start)["channel"], "binding");
    // The page's own world sees nothing new.
    assert_eq!(
        eval_in(&mut rec, "Object.getOwnPropertyNames(window).filter(n => /teach|^b[0-9a-f]{12}$/i.test(n)).length").await,
        json!(0)
    );

    // The daemon's own input is trusted CDP input: a stand-in for the user.
    for cmd in [
        json!({"id": "a1", "action": "fill", "selector": "#name", "value": "Ada"}),
        json!({"id": "a2", "action": "fill", "selector": "#pw", "value": "hunter2"}),
        json!({"id": "a3", "action": "click", "selector": "#gift"}),
        json!({"id": "a4", "action": "click", "selector": "#go"}),
    ] {
        assert_success(&run_cmd(&mut rec, cmd).await);
    }
    assert_success(
        &run_cmd(
            &mut rec,
            json!({"id": "a5", "action": "waitforurl", "url": "/done"}),
        )
        .await,
    );
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    let stop = run_cmd(&mut rec, json!({"id": "t2", "action": "teach_stop"})).await;
    assert_success(&stop);
    let recording = get_data(&stop)["recording"].clone();
    assert!(
        !recording.to_string().contains("hunter2"),
        "a password reached the recording: {recording}"
    );
    let compiled = crate::teach::compile(&recording, "order").expect("compiles");
    assert_eq!(compiled.params.len(), 1, "{:?}", compiled.steps);
    let pw = compiled.params[0].name.clone();
    let text = compiled.program.to_string();
    assert!(text.contains("\"Ada\""), "{text}");
    assert!(text.contains("waitforurl"), "{text}");
    // After stop the recorder is gone: no binding, and nothing else to stop.
    let again = run_cmd(&mut rec, json!({"id": "t3", "action": "teach_stop"})).await;
    assert_eq!(again["success"], false);
    close_state(&mut rec).await;

    // Replay in a fresh browser.
    let mut play = DaemonState::new();
    launch_on(port, &mut play).await;
    let mut args = serde_json::Map::new();
    args.insert(pw, json!("s3cret"));
    let ran = run_cmd(
        &mut play,
        json!({"id": "r1", "action": "script", "program": compiled.program, "args": args, "timeout_ms": 60000}),
    )
    .await;
    assert_success(&ran);
    assert_eq!(get_data(&ran)["ok"], true, "{}", get_data(&ran));
    assert_eq!(
        eval_in(&mut play, "document.getElementById('out').textContent").await,
        json!("ordered by Ada pw=6 gift=yes")
    );
    close_state(&mut play).await;
    server.abort();
}

#[tokio::test]
#[ignore]
async fn e2e_teach_refuses_a_second_start_and_a_stop_without_start() {
    let (port, server) = spawn_html_server(TEACH_FIXTURE.to_string()).await;
    let mut state = DaemonState::new();
    launch_on(port, &mut state).await;
    let stop = run_cmd(&mut state, json!({"id": "s0", "action": "teach_stop"})).await;
    assert_eq!(stop["success"], false);
    assert_success(&run_cmd(&mut state, json!({"id": "s1", "action": "teach_start"})).await);
    let twice = run_cmd(&mut state, json!({"id": "s2", "action": "teach_start"})).await;
    assert_eq!(twice["success"], false);
    let cancel = run_cmd(
        &mut state,
        json!({"id": "s3", "action": "teach_stop", "discard": true}),
    )
    .await;
    assert_success(&cancel);
    assert_eq!(get_data(&cancel)["recording"], Value::Null);
    close_state(&mut state).await;
    server.abort();
}
