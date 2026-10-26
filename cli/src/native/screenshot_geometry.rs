//! Screenshot boxes use protocol geometry, without evaluating page methods.
//! DOM.getBoxModel returns CSS viewport quads for the session's renderer root.
//! Same-process descendants already include their frame offsets. Out-of-process
//! frames must be measured in their own session and projected through each owner.
use super::{CdpClient, HashMap, Rect, Value};
use serde_json::json;
use std::collections::HashSet;

#[derive(Clone)]
struct Frame {
    parent: Option<String>,
    session: String,
}

/// Homogeneous mapping from a renderer viewport into the top viewport.
/// Keeping the full matrix preserves rotation, reflection, skew and perspective
/// when crossing nested renderer boundaries; bounding boxes are formed last.
#[derive(Clone, Copy, Debug)]
struct Projection {
    matrix: [[f64; 3]; 3],
}

impl Projection {
    const IDENTITY: Self = Self {
        matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    fn compose(self, child: Self) -> Result<Self, String> {
        let mut matrix = [[0.0; 3]; 3];
        for (row, values) in matrix.iter_mut().enumerate() {
            for (col, value) in values.iter_mut().enumerate() {
                *value = (0..3)
                    .map(|i| self.matrix[row][i] * child.matrix[i][col])
                    .sum();
            }
        }
        if matrix.iter().flatten().any(|v| !v.is_finite()) {
            return Err("Screenshot: non-finite iframe projection".into());
        }
        Ok(Self { matrix })
    }

    fn denominator(self, x: f64, y: f64) -> Result<f64, String> {
        let terms = [
            self.matrix[2][0] * x,
            self.matrix[2][1] * y,
            self.matrix[2][2],
        ];
        let denominator: f64 = terms.iter().sum();
        let magnitude: f64 = terms.iter().map(|v| v.abs()).sum();
        if !denominator.is_finite() || denominator.abs() <= 1e-10 * magnitude {
            return Err("Screenshot: iframe projection approaches the perspective horizon".into());
        }
        Ok(denominator)
    }

    fn point(self, x: f64, y: f64) -> Result<[f64; 2], String> {
        let denominator = self.denominator(x, y)?;
        let point = [
            (self.matrix[0][0] * x + self.matrix[0][1] * y + self.matrix[0][2]) / denominator,
            (self.matrix[1][0] * x + self.matrix[1][1] * y + self.matrix[1][2]) / denominator,
        ];
        if point.iter().any(|v| !v.is_finite()) {
            return Err("Screenshot: non-finite projected coordinates".into());
        }
        Ok(point)
    }

    fn rect(self, quad: &Value) -> Result<Rect, String> {
        let quad = quad_points(quad)?;
        let mut points = Vec::with_capacity(4);
        let mut sign = None;
        for p in quad.as_chunks::<2>().0 {
            let positive = self.denominator(p[0], p[1])? > 0.0;
            if sign.is_some_and(|previous| previous != positive) {
                return Err("Screenshot: iframe box crosses the perspective horizon".into());
            }
            sign = Some(positive);
            points.push(self.point(p[0], p[1])?);
        }
        let x = points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let y = points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        let width = points
            .iter()
            .map(|p| p[0])
            .fold(f64::NEG_INFINITY, f64::max)
            - x;
        let height = points
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max)
            - y;
        if !width.is_finite() || !height.is_finite() {
            return Err("Screenshot: non-finite projected bounds".into());
        }
        Ok(Rect {
            x,
            y,
            width,
            height,
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
    let invalid = "Screenshot: cannot safely project this iframe transform or viewport";
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(invalid.into());
    }
    // Solve the unit-square-to-quad homography in translated, normalized
    // coordinates, avoiding loss of precision from large page offsets. CDP
    // supplies ordered content corners, so borders are already excluded.
    let scale = q
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|p| [(p[0] - q[0]).abs(), (p[1] - q[1]).abs()])
        .fold(0.0_f64, f64::max);
    if !scale.is_finite() || scale <= 0.0 {
        return Err(invalid.into());
    }
    let points: Vec<_> = q
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| [(p[0] - q[0]) / scale, (p[1] - q[1]) / scale])
        .collect();
    // Both windings are valid (mirrored owners). Concave, crossing, edge-on
    // and almost collapsed quads cannot establish a stable visible plane.
    let mut winding = None;
    for i in 0..4 {
        let a = points[i];
        let b = points[(i + 1) % 4];
        let c = points[(i + 2) % 4];
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() <= 1e-10 || winding.is_some_and(|sign| sign != (cross > 0.0)) {
            return Err(invalid.into());
        }
        winding = Some(cross > 0.0);
    }
    let [x1, y1] = points[1];
    let [x2, y2] = points[2];
    let [x3, y3] = points[3];
    let dx1 = x1 - x2;
    let dx2 = x3 - x2;
    let dx3 = -x1 + x2 - x3;
    let dy1 = y1 - y2;
    let dy2 = y3 - y2;
    let dy3 = -y1 + y2 - y3;
    let determinant = dx1 * dy2 - dx2 * dy1;
    if determinant.abs() <= 1e-10 {
        return Err(invalid.into());
    }
    let g = (dx3 * dy2 - dx2 * dy3) / determinant;
    let h = (dx1 * dy3 - dx3 * dy1) / determinant;
    // Affine quads need no normalization round trip. Preserve exact simple
    // translations/scales and avoid introducing rounding at pixel boundaries.
    if g == 0.0 && h == 0.0 {
        return Ok(Projection {
            matrix: [
                [(q[2] - q[0]) / width, (q[6] - q[0]) / height, q[0]],
                [(q[3] - q[1]) / width, (q[7] - q[1]) / height, q[1]],
                [0.0, 0.0, 1.0],
            ],
        });
    }
    let normalized = Projection {
        matrix: [
            [x1 * (1.0 + g) / width, x3 * (1.0 + h) / height, 0.0],
            [y1 * (1.0 + g) / width, y3 * (1.0 + h) / height, 0.0],
            [g / width, h / height, 1.0],
        ],
    };
    let projection = Projection {
        matrix: [[scale, 0.0, q[0]], [0.0, scale, q[1]], [0.0, 0.0, 1.0]],
    }
    .compose(normalized)?;
    // A valid viewport must stay on one side of the perspective horizon.
    // Check the four supplied correspondences as a final conditioning guard.
    for (i, [x, y]) in [[0.0, 0.0], [width, 0.0], [width, height], [0.0, height]]
        .into_iter()
        .enumerate()
    {
        if projection.denominator(x, y)? <= 0.0 {
            return Err(invalid.into());
        }
        let actual = projection.point(x, y)?;
        if (actual[0] - q[2 * i]).abs() > scale * 1e-8
            || (actual[1] - q[2 * i + 1]).abs() > scale * 1e-8
        {
            return Err(invalid.into());
        }
    }
    Ok(projection)
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
        target_frames: &[Option<&str>],
        target_session: Option<&str>,
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
                .await;
            // The session map can outlive a detached frame while snapshot
            // work is in flight. An unrelated stale renderer must not prevent
            // annotations elsewhere; required renderers are checked below.
            let Ok(tree) = tree else {
                continue;
            };
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
        // Only project renderers that contribute labels/the selector and their
        // ancestors. An unrelated hidden iframe has no owner box and must not
        // prevent capturing a visible target elsewhere in the page.
        let mut required = HashSet::from([top_session.to_owned()]);
        if let Some(session) = target_session {
            required.insert(session.to_owned());
        }
        for frame in target_frames.iter().flatten() {
            let frame = frames
                .get(*frame)
                .ok_or("Screenshot: reference frame unavailable")?;
            required.insert(frame.session.clone());
        }
        loop {
            let previous = required.len();
            for session in required.clone() {
                if let Some(root) = roots.get(&session) {
                    let parent = frames
                        .get(root)
                        .and_then(|f| f.parent.as_ref())
                        .and_then(|id| frames.get(id))
                        .ok_or("Screenshot: iframe parent unavailable")?;
                    required.insert(parent.session.clone());
                }
            }
            if required.len() == previous {
                break;
            }
        }
        if required
            .iter()
            .any(|session| session != top_session && !roots.contains_key(session))
        {
            return Err("Screenshot: reference frame unavailable".into());
        }
        roots.retain(|session, _| required.contains(session));
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
                projections.insert(session.clone(), parent_projection.compose(projection)?);
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

    fn assert_point(actual: [f64; 2], expected: [f64; 2]) {
        for (a, e) in actual.into_iter().zip(expected) {
            assert!(
                (a - e).abs() < 1e-8,
                "expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn maps_rotated_skewed_and_mirrored_owner_interiors() {
        let metrics = json!({"cssLayoutViewport":{"clientWidth":100,"clientHeight":100}});
        for (quad, expected) in [
            (
                json!([300, 100, 300, 200, 200, 200, 200, 100]),
                [240.0, 125.0],
            ),
            (
                json!([100, 100, 300, 150, 350, 250, 150, 200]),
                [180.0, 172.5],
            ),
            (
                json!([300, 100, 100, 100, 100, 200, 300, 200]),
                [250.0, 160.0],
            ),
        ] {
            let projection = owner_projection(&quad, &metrics).unwrap();
            assert_point(projection.point(25.0, 60.0).unwrap(), expected);
        }
    }

    #[test]
    fn perspective_maps_interior_not_bounding_box_interpolation() {
        let p = owner_projection(
            &json!([100, 100, 300, 100, 260, 180, 140, 180]),
            &json!({"cssLayoutViewport":{"clientWidth":100,"clientHeight":100}}),
        )
        .unwrap();
        // The image of the center is the intersection of the diagonals,
        // not the arithmetic mean of the projected corners (200, 140).
        assert_point(p.point(50.0, 50.0).unwrap(), [200.0, 150.0]);
        assert_point(p.point(0.0, 50.0).unwrap(), [125.0, 150.0]);
        let rect = p.rect(&json!([25, 25, 75, 25, 75, 75, 25, 75])).unwrap();
        assert_point([rect.x, rect.y], [1100.0 / 7.0, 900.0 / 7.0]);
        assert_point([rect.width, rect.height], [600.0 / 7.0, 800.0 / 21.0]);
    }

    #[test]
    fn nested_projection_preserves_corners_before_forming_bounds() {
        let metrics = json!({"cssLayoutViewport":{"clientWidth":100,"clientHeight":100}});
        let parent =
            owner_projection(&json!([100, 100, 300, 100, 260, 180, 140, 180]), &metrics).unwrap();
        let child = owner_projection(&json!([100, 0, 100, 100, 0, 100, 0, 0]), &metrics).unwrap();
        let nested = parent.compose(child).unwrap();
        assert_point(nested.point(50.0, 50.0).unwrap(), [200.0, 150.0]);
        assert_point(nested.point(50.0, 100.0).unwrap(), [125.0, 150.0]);
    }

    #[test]
    fn refuses_collapsed_concave_and_horizon_geometry() {
        let metrics = json!({"cssLayoutViewport":{"clientWidth":100,"clientHeight":100}});
        for quad in [
            json!([0, 0, 100, 0, 100, 0, 0, 0]),
            json!([0, 0, 100, 0, 10, 10, 0, 100]),
            json!([0, 0, 100, 0, 100, 0.000000001, 0, 0.000000001]),
        ] {
            assert!(owner_projection(&quad, &metrics).is_err(), "{quad}");
        }
        let p = Projection {
            matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.01, 0.0, 1.0]],
        };
        assert!(p
            .rect(&json!([-110, 0, -90, 0, -90, 10, -110, 10]))
            .is_err());
        assert!(p.point(-100.0, 0.0).is_err());
    }

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
        let nested = p
            .compose(Projection {
                matrix: [[0.5, 0.0, 10.0], [0.0, 0.5, 20.0], [0.0, 0.0, 1.0]],
            })
            .unwrap();
        let rect = nested.rect(&json!([8, 8, 92, 8, 92, 29, 8, 29])).unwrap();
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height),
            (498.0, 106.0, 84.0, 21.0)
        );
    }

    #[test]
    fn refuses_unsupported_frame_geometry() {
        let metrics = json!({"cssLayoutViewport":{"clientWidth":100,"clientHeight":100}});
        assert!(owner_projection(&json!([0, 0, 100, 100, 100, 0, 0, 100]), &metrics).is_err());
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
                if sid == "stale-session" {
                    assert_eq!(request["method"], "Page.getFrameTree");
                    ws.send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"id":request["id"],"error":{"code":-32000,"message":"Session with given id not found"}}).to_string(),
                    ))
                    .await
                    .unwrap();
                    continue;
                }
                let result = match request["method"].as_str().unwrap() {
                    "Page.getFrameTree" if sid == "main" => {
                        json!({"frameTree":{"frame":{"id":"top"},"childFrames":[{"frame":{"id":"same"}},{"frame":{"id":"cross"}}]}})
                    }
                    "Page.getFrameTree" if sid == "oop" => {
                        json!({"frameTree":{"frame":{"id":"cross","parentId":"top"}}})
                    }
                    "Page.getFrameTree" if sid == "hidden-session" => {
                        json!({"frameTree":{"frame":{"id":"hidden","parentId":"top"}}})
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
            &HashMap::from([
                ("cross".into(), "oop".into()),
                ("hidden".into(), "hidden-session".into()),
                ("stale".into(), "stale-session".into()),
            ]),
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

    #[tokio::test]
    async fn required_stale_renderer_refuses_annotation_and_selector_geometry() {
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
                assert_eq!(request["method"], "Page.getFrameTree");
                let response = if request["sessionId"] == "main" {
                    json!({"id":request["id"],"result":{"frameTree":{"frame":{"id":"top"},"childFrames":[{"frame":{"id":"stale"}}]}}})
                } else {
                    assert_eq!(request["sessionId"], "stale-session");
                    json!({"id":request["id"],"error":{"code":-32000,"message":"Session with given id not found"}})
                };
                if ws
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        response.to_string(),
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
        let sessions = HashMap::from([("stale".into(), "stale-session".into())]);
        // A stale annotation frame and a stale selector session both require
        // refusal. Neither may silently inherit the top renderer's geometry.
        for (frame, session) in [(Some("stale"), None), (None, Some("stale-session"))] {
            let result = FrameGeometry::load(&client, "main", &sessions, &[frame], session).await;
            assert_eq!(
                result.err().as_deref(),
                Some("Screenshot: reference frame unavailable")
            );
        }
        server.abort();
    }

    fn return_model(quad: [f64; 8]) -> Value {
        json!({"model":{"content":quad,"border":quad}})
    }
}
