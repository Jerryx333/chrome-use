//! Native double-click delivery and refusal on fixture pages.
use super::*;

#[tokio::test]
#[ignore]
async fn e2e_double_click_delivers_native_sequence_and_refuses_covered_target() {
    let html = r##"<!doctype html><button id="target">Target</button>
      <div id="cover" style="display:none;position:fixed;inset:0;z-index:10"></div>
      <script>
      window.clickEvents=[];
      for (const type of ['mousedown','mouseup','click','dblclick'])
        document.getElementById('target').addEventListener(type,e=>
          window.clickEvents.push({type:e.type,detail:e.detail,trusted:e.isTrusted}));
      </script>"##;
    let (port, server) = spawn_html_server(html.to_string()).await;
    let mut state = DaemonState::new();
    launch_on(port, &mut state).await;
    let response = run_cmd(
        &mut state,
        json!({"id":"double","action":"dblclick","selector":"#target"}),
    )
    .await;
    assert_success(&response);
    assert_eq!(get_data(&response)["verified"], true);
    let events = eval_in(&mut state, "window.clickEvents").await;
    assert_eq!(
        events,
        json!([
            {"type":"mousedown","detail":1,"trusted":true},
            {"type":"mouseup","detail":1,"trusted":true},
            {"type":"click","detail":1,"trusted":true},
            {"type":"mousedown","detail":2,"trusted":true},
            {"type":"mouseup","detail":2,"trusted":true},
            {"type":"click","detail":2,"trusted":true},
            {"type":"dblclick","detail":2,"trusted":true},
        ])
    );
    eval_in(
        &mut state,
        "window.clickEvents=[];document.getElementById('cover').style.display='block'",
    )
    .await;
    let response = run_cmd(
        &mut state,
        json!({"id":"covered","action":"dblclick","selector":"#target"}),
    )
    .await;
    assert_eq!(response["success"], false);
    assert_eq!(eval_in(&mut state, "window.clickEvents.length").await, 0);
    run_cmd(&mut state, json!({"id":"close","action":"close"})).await;
    server.abort();
}
