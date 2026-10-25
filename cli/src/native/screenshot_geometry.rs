//! Screenshot boxes use protocol geometry, without evaluating page methods.
//! DOM.getBoxModel returns CSS viewport quads for the session's renderer root.
//! Same-process descendants already include their frame offsets. Out-of-process
//! frames must be measured in their own session and projected through each owner.
use super::{CdpClient, HashMap, Rect, Value};
use serde_json::json;

#[derive(Clone)]
struct Frame {
    parent: Option<String>,
    session: String,
}

#[derive(Clone, Copy, Debug)]
struct Projection {
    x: f64,
    y: f64,
    sx: f64,
    sy: f64,
}

impl Projection {
    const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        sx: 1.0,
        sy: 1.0,
    };

    fn compose(self, child: Self) -> Self {
        Self {
            x: self.x + child.x * self.sx,
            y: self.y + child.y * self.sy,
            sx: self.sx * child.sx,
            sy: self.sy * child.sy,
        }
    }

    fn rect(self, quad: &Value) -> Result<Rect, String> {
        let quad = quad_points(quad)?;
        let xs: Vec<_> = quad
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| self.x + p[0] * self.sx)
            .collect();
        let ys: Vec<_> = quad
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| self.y + p[1] * self.sy)
            .collect();
        let x = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let y = ys.iter().copied().fold(f64::INFINITY, f64::min);
        Ok(Rect {
            x,
            y,
            width: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max) - x,
            height: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max) - y,
        })
    }
}

fn quad_points(value: &Value) -> Result<Vec<f64>, String> {
    let points: Option<Vec<_>> = value
        .as_array()
        .ok_or("Screenshot: invalid protocol box quad")?
        .iter()
        .map(Value::as_f64)
        .collect();
    let points = points.ok_or("Screenshot: invalid protocol box quad")?;
    if points.len() != 8 || points.iter().any(|v| !v.is_finite()) {
        return Err("Screenshot: invalid protocol box quad".into());
    }
    Ok(points)
}

fn owner_projection(quad: &Value, metrics: &Value) -> Result<Projection, String> {
    let q = quad_points(quad)?;
    let viewport = &metrics["cssLayoutViewport"];
    let width = viewport["clientWidth"].as_f64().unwrap_or(0.0);
    let height = viewport["clientHeight"].as_f64().unwrap_or(0.0);
    // Translate and scale axis-aligned owners. Refuse rotation/perspective
    // rather than placing labels using an unverified frame transformation.
    if width <= 0.0
        || height <= 0.0
        || (q[1] - q[3]).abs() > 0.01
        || (q[2] - q[4]).abs() > 0.01
        || (q[5] - q[7]).abs() > 0.01
        || (q[6] - q[0]).abs() > 0.01
        || q[2] <= q[0]
        || q[5] <= q[1]
    {
        return Err("Screenshot: cannot safely project this iframe transform or viewport".into());
    }
    Ok(Projection {
        x: q[0],
        y: q[1],
        sx: (q[2] - q[0]) / width,
        sy: (q[5] - q[1]) / height,
    })
}

pub(super) struct FrameGeometry {
    top_session: String,
    frames: HashMap<String, Frame>,
    projections: HashMap<String, Projection>,
}

fn add_tree(
    tree: &Value,
    session: &str,
    parent: Option<&str>,
    frames: &mut HashMap<String, Frame>,
) -> Result<String, String> {
    let frame = &tree["frame"];
    let id = frame["id"]
        .as_str()
        .ok_or("Screenshot: frame id unavailable")?;
    let parent = frame["parentId"]
        .as_str()
        .or(parent)
        .map(str::to_owned)
        .or_else(|| frames.get(id).and_then(|f| f.parent.clone()));
    frames.insert(
        id.into(),
        Frame {
            parent,
            session: session.into(),
        },
    );
    if let Some(children) = tree["childFrames"].as_array() {
        for child in children {
            add_tree(child, session, Some(id), frames)?;
        }
    }
    Ok(id.into())
}

impl FrameGeometry {
    pub(super) async fn load(
        client: &CdpClient,
        top_session: &str,
        iframe_sessions: &HashMap<String, String>,
    ) -> Result<Self, String> {
        let tree = client
            .send_command_no_params("Page.getFrameTree", Some(top_session))
            .await?;
        let mut frames = HashMap::new();
        add_tree(&tree["frameTree"], top_session, None, &mut frames)?;
        let mut roots = HashMap::new();
        for session in iframe_sessions.values() {
            if session == top_session || roots.contains_key(session) {
                continue;
            }
            let tree = client
                .send_command_no_params("Page.getFrameTree", Some(session))
                .await?;
            let root = add_tree(&tree["frameTree"], session, None, &mut frames)?;
            roots.insert(session.clone(), root);
        }
        // A parent's tree can contain an OOP placeholder. The explicit session
        // map wins independent of HashMap traversal order.
        for (frame_id, session) in iframe_sessions {
            if let Some(frame) = frames.get_mut(frame_id) {
                frame.session = session.clone();
            }
        }
        let mut projections = HashMap::from([(top_session.to_owned(), Projection::IDENTITY)]);
        while !roots.is_empty() {
            let previous = roots.len();
            for (session, root) in roots.clone() {
                let parent = frames
                    .get(&root)
                    .and_then(|f| f.parent.as_ref())
                    .and_then(|id| frames.get(id))
                    .ok_or("Screenshot: iframe parent unavailable")?;
                let Some(parent_projection) = projections.get(&parent.session).copied() else {
                    continue;
                };
                let owner = client
                    .send_command(
                        "DOM.getFrameOwner",
                        Some(json!({"frameId":root})),
                        Some(&parent.session),
                    )
                    .await?;
                let backend = owner["backendNodeId"]
                    .as_i64()
                    .ok_or("Screenshot: iframe owner unavailable")?;
                let model = client
                    .send_command(
                        "DOM.getBoxModel",
                        Some(json!({"backendNodeId":backend})),
                        Some(&parent.session),
                    )
                    .await?;
                let metrics = client
                    .send_command_no_params("Page.getLayoutMetrics", Some(&session))
                    .await?;
                let projection = owner_projection(&model["model"]["content"], &metrics)?;
                projections.insert(session.clone(), parent_projection.compose(projection));
                roots.remove(&session);
            }
            if roots.len() == previous {
                return Err("Screenshot: iframe ancestry unavailable".into());
            }
        }
        Ok(Self {
            top_session: top_session.into(),
            frames,
            projections,
        })
    }

    pub(super) fn session_for(&self, frame: Option<&str>) -> Result<&str, String> {
        match frame {
            None => Ok(&self.top_session),
            Some(frame) => self
                .frames
                .get(frame)
                .map(|f| f.session.as_str())
                .ok_or_else(|| {
                    "Screenshot: reference frame is unavailable; take a fresh snapshot".into()
                }),
        }
    }

    pub(super) fn rect(&self, session: &str, quad: &Value) -> Result<Rect, String> {
        self.projections
            .get(session)
            .ok_or("Screenshot: frame projection unavailable")?
            .rect(quad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_oop_boxes_through_content_border_scroll_and_scale() {
        let p = owner_projection(
            &json!([470, 58, 1350, 58, 1350, 418, 470, 418]),
            &json!({"cssLayoutViewport":{"clientWidth":440,"clientHeight":180}}),
        )
        .unwrap();
        let rect = p.rect(&json!([8, 8, 92, 8, 92, 29, 8, 29])).unwrap();
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height),
            (486.0, 74.0, 168.0, 42.0)
        );
        let nested = p.compose(Projection {
            x: 10.0,
            y: 20.0,
            sx: 0.5,
            sy: 0.5,
        });
        let rect = nested.rect(&json!([8, 8, 92, 8, 92, 29, 8, 29])).unwrap();
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height),
            (498.0, 106.0, 84.0, 21.0)
        );
    }

    #[test]
    fn refuses_unsupported_frame_geometry() {
        let metrics = json!({"cssLayoutViewport":{"clientWidth":100,"clientHeight":100}});
        assert!(owner_projection(&json!([0, 0, 100, 10, 90, 110, 0, 100]), &metrics).is_err());
        assert!(owner_projection(&json!([0, 0, 100, 0, 100, 100, 0, 100]), &json!({})).is_err());
        assert!(Projection::IDENTITY.rect(&json!([1, 2])).is_err());
    }

    #[test]
    fn frame_ids_choose_renderer_and_never_guess_unknown_frame() {
        let mut frames = HashMap::new();
        add_tree(
            &json!({"frame":{"id":"top"},"childFrames":[{"frame":{"id":"same"}}]}),
            "main",
            None,
            &mut frames,
        )
        .unwrap();
        add_tree(
            &json!({"frame":{"id":"cross","parentId":"top"}}),
            "oop",
            None,
            &mut frames,
        )
        .unwrap();
        let geometry = FrameGeometry {
            top_session: "main".into(),
            frames,
            projections: HashMap::new(),
        };
        assert_eq!(geometry.session_for(Some("same")).unwrap(), "main");
        assert_eq!(geometry.session_for(Some("cross")).unwrap(), "oop");
        assert!(geometry.session_for(Some("gone")).is_err());
    }

    #[tokio::test]
    async fn annotations_resolve_colliding_backend_ids_in_their_own_renderer() {
        use futures_util::{SinkExt, StreamExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(message)) = ws.next().await {
                let Ok(text) = message.into_text() else {
                    continue;
                };
                let request: Value = serde_json::from_str(&text).unwrap();
                let sid = request["sessionId"].as_str().unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "Page.getFrameTree" if sid == "main" => {
                        json!({"frameTree":{"frame":{"id":"top"},"childFrames":[{"frame":{"id":"same"}},{"frame":{"id":"cross"}}]}})
                    }
                    "Page.getFrameTree" if sid == "oop" => {
                        json!({"frameTree":{"frame":{"id":"cross","parentId":"top"}}})
                    }
                    "Page.getLayoutMetrics" if sid == "oop" => {
                        json!({"cssLayoutViewport":{"clientWidth":440,"clientHeight":180}})
                    }
                    "DOM.getFrameOwner" => {
                        assert_eq!(sid, "main");
                        assert_eq!(request["params"]["frameId"], "cross");
                        json!({"backendNodeId":99})
                    }
                    "DOM.getBoxModel" => {
                        match (sid, request["params"]["backendNodeId"].as_i64().unwrap()) {
                            ("main", 99) => return_model([
                                470.0, 128.0, 910.0, 128.0, 910.0, 308.0, 470.0, 308.0,
                            ]),
                            ("main", 7) => {
                                return_model([10.0, 20.0, 20.0, 20.0, 20.0, 30.0, 10.0, 30.0])
                            }
                            ("main", 8) => {
                                return_model([34.0, 136.0, 118.0, 136.0, 118.0, 157.0, 34.0, 157.0])
                            }
                            ("oop", 7) => {
                                return_model([8.0, 8.0, 92.0, 8.0, 92.0, 29.0, 8.0, 29.0])
                            }
                            other => panic!("wrong renderer/node: {other:?}"),
                        }
                    }
                    other => {
                        panic!("annotation measurement must not evaluate page JavaScript: {other}")
                    }
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
        let mut refs = super::super::RefMap::new();
        refs.add_with_frame("e1".into(), Some(7), "button", "top", None, Some("top"));
        refs.add_with_frame("e2".into(), Some(8), "button", "same", None, Some("same"));
        refs.add_with_frame("e3".into(), Some(7), "button", "cross", None, Some("cross"));
        let annotations = super::super::collect_annotations(
            &client,
            "main",
            &refs,
            &HashMap::from([("cross".into(), "oop".into())]),
        )
        .await
        .unwrap();
        let observed: Vec<_> = annotations
            .iter()
            .map(|a| (a.name.as_deref().unwrap(), a.rect.x, a.rect.y))
            .collect();
        assert_eq!(
            observed,
            [
                ("top", 10.0, 20.0),
                ("same", 34.0, 136.0),
                ("cross", 478.0, 136.0)
            ]
        );
        server.abort();
    }

    fn return_model(quad: [f64; 8]) -> Value {
        json!({"model":{"content":quad,"border":quad}})
    }
}
