//! Annotated captures must not add nodes or styles to the page.
use super::*;

#[tokio::test]
#[ignore]
async fn e2e_screenshot_annotations_leave_no_dom_mutations() {
    let html = r##"<!doctype html><meta charset="utf-8">
      <style>body{margin:0;height:1800px}button{position:absolute;left:30px;top:800px;width:90px;height:30px}</style>
      <button id="target">Target</button>
      <script>
      window.annotationMutations = [];
      new MutationObserver(records => window.annotationMutations.push(...records))
        .observe(document.documentElement, {subtree:true,childList:true,attributes:true});
      </script>"##;
    let (port, server) = spawn_html_server(html.to_string()).await;
    let mut state = DaemonState::new();
    launch_on(port, &mut state).await;
    eval_in(&mut state, "window.scrollTo(0,600)").await;
    for (index, extra) in [
        json!({}),
        json!({"fullPage":true}),
        json!({"selector":"#target"}),
        json!({"clip":{"x":20,"y":790,"width":120,"height":60}}),
    ]
    .iter()
    .enumerate()
    {
        eval_in(&mut state, "window.annotationMutations=[]").await;
        let mut command = json!({"id":format!("shot-{index}"),"action":"screenshot","annotate":true,"base64":true});
        for (key, value) in extra.as_object().unwrap() {
            command[key] = value.clone();
        }
        let response = run_cmd(&mut state, command).await;
        assert_success(&response);
        assert_eq!(
            eval_in(&mut state, "window.annotationMutations.length").await,
            0
        );
        let data = get_data(&response);
        assert!(
            !data["annotations"].as_array().unwrap().is_empty(),
            "missing labels: {response}"
        );
        let pixels = STANDARD.decode(data["base64"].as_str().unwrap()).unwrap();
        let image = image::load_from_memory(&pixels).unwrap().to_rgb8();
        assert!(
            image.pixels().any(|p| p.0 == [230, 32, 32]),
            "no rendered labels"
        );
        if let Some(path) = data["path"].as_str() {
            let _ = std::fs::remove_file(path);
        }
    }
    run_cmd(&mut state, json!({"id":"close","action":"close"})).await;
    server.abort();
}
