//! Annotated captures must not add nodes or styles to the page.
use super::*;

#[tokio::test]
#[ignore]
async fn e2e_screenshot_annotations_leave_no_dom_mutations() {
    let child =
        "<!doctype html><style>button{width:90px;height:30px}</style><button>Child</button>";
    let (child_port, child_server) = spawn_html_server(child.to_string()).await;
    let html = r##"<!doctype html><meta charset="utf-8">
      <style>body{margin:0;height:1800px}button{position:absolute;left:30px;top:800px;width:90px;height:30px}</style>
      <button id="target">Target</button>
      <iframe style="position:absolute;left:200px;top:800px;width:160px;height:100px;border:3px solid" srcdoc="<!doctype html><style>button{width:90px;height:30px}</style><button>Child</button>"></iframe>
      <iframe style="position:absolute;left:400px;top:800px;width:160px;height:100px;border:3px solid" src="http://localhost:__CHILD_PORT__/"></iframe>
      <script>
      window.annotationMutations = [];
      new MutationObserver(records => window.annotationMutations.push(...records))
        .observe(document.documentElement, {subtree:true,childList:true,attributes:true});
      </script>"##;
    let (port, server) =
        spawn_html_server(html.replace("__CHILD_PORT__", &child_port.to_string())).await;
    let mut state = DaemonState::new();
    launch_on(port, &mut state).await;
    for scale in [1.0, 2.0] {
        let viewport = run_cmd(&mut state, json!({"id":"viewport","action":"viewport","width":1000,"height":700,"deviceScaleFactor":scale})).await;
        assert_success(&viewport);
        eval_in(&mut state, "window.scrollTo(0,600)").await;
        for (index, extra) in [
            json!({}),
            json!({"fullPage":true}),
            json!({"selector":"#target"}),
            json!({"clip":{"x":20,"y":790,"width":600,"height":200}}),
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
            let annotations = data["annotations"].as_array().unwrap();
            let target = annotations
                .iter()
                .find(|a| a["name"] == "Target")
                .expect("target label");
            let (x, y) = [(30, 200), (30, 800), (0, 0), (10, 10)][index];
            assert_eq!(
                target["box"],
                json!({"x":x,"y":y,"width":90,"height":30}),
                "scale {scale}, capture {index}"
            );
            if index != 2 {
                let mut children: Vec<_> = annotations
                    .iter()
                    .filter(|a| a["name"] == "Child")
                    .map(|a| {
                        (
                            a["box"]["x"].as_i64().unwrap(),
                            a["box"]["y"].as_i64().unwrap(),
                        )
                    })
                    .collect();
                children.sort();
                let (dx, dy) = [(0, 600), (0, 0), (0, 0), (20, 790)][index];
                assert_eq!(
                    children,
                    [(211 - dx, 811 - dy), (411 - dx, 811 - dy)],
                    "scale {scale}, capture {index}"
                );
            }
            let pixels = STANDARD.decode(data["base64"].as_str().unwrap()).unwrap();
            let image = image::load_from_memory(&pixels).unwrap().to_rgb8();
            let viewport_width = eval_in(&mut state, "visualViewport.width")
                .await
                .as_f64()
                .unwrap();
            let content_width = eval_in(&mut state, "document.documentElement.scrollWidth")
                .await
                .as_f64()
                .unwrap();
            let css_width = [viewport_width, content_width, 90.0, 600.0][index];
            let raster_scale = image.width() as f64 / css_width;
            assert_eq!(raster_scale, scale, "capture scale {index}");
            assert_eq!(
                image
                    .get_pixel(
                        (x as f64 * raster_scale) as u32,
                        ((y as f64 + 15.0) * raster_scale) as u32
                    )
                    .0,
                [230, 32, 32],
                "label border at capture coordinates"
            );
            assert!(
                image.pixels().any(|p| p.0 == [230, 32, 32]),
                "no rendered labels"
            );
            if let Some(path) = data["path"].as_str() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    run_cmd(&mut state, json!({"id":"close","action":"close"})).await;
    server.abort();
    child_server.abort();
}
