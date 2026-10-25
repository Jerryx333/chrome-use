use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

use std::collections::HashMap;

use super::cdp::client::CdpClient;
use super::cdp::types::*;
use super::element::RefMap;

#[path = "screenshot_annotations.rs"]
mod annotation_image;
#[path = "screenshot_geometry.rs"]
mod geometry;

#[derive(Debug, Clone)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone)]
struct RawAnnotation {
    ref_id: String,
    number: u64,
    role: String,
    name: Option<String>,
    rect: Rect,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnnotationBox {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone)]
pub struct ScreenshotAnnotation {
    pub ref_id: String,
    pub number: u64,
    pub role: String,
    pub name: Option<String>,
    pub box_: AnnotationBox,
}

#[derive(Debug, Clone)]
pub struct ScreenshotResult {
    pub path: String,
    pub base64: String,
    pub annotations: Vec<ScreenshotAnnotation>,
}

#[derive(Debug, Clone)]
pub struct ScreenshotOptions {
    pub selector: Option<String>,
    pub path: Option<String>,
    pub full_page: bool,
    pub format: String,
    pub quality: Option<i32>,
    pub annotate: bool,
    pub output_dir: Option<String>,
    /// Explicit pixel region (x, y, width, height) — `--clip` (issue #34). Takes
    /// precedence over selector/full_page.
    pub clip: Option<(f64, f64, f64, f64)>,
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            selector: None,
            path: None,
            full_page: false,
            format: "png".to_string(),
            quality: None,
            annotate: false,
            output_dir: None,
            clip: None,
        }
    }
}

impl Serialize for ScreenshotAnnotation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("ScreenshotAnnotation", 5)?;
        state.serialize_field("ref", &self.ref_id)?;
        state.serialize_field("number", &self.number)?;
        state.serialize_field("role", &self.role)?;
        if let Some(name) = &self.name {
            state.serialize_field("name", name)?;
        }
        state.serialize_field("box", &self.box_)?;
        state.end()
    }
}

/// Captures via CDP, then draws optional labels into the image in the CLI.
/// Annotation rendering never inserts DOM nodes or styles into the page.
pub async fn take_screenshot(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    options: &ScreenshotOptions,
    iframe_sessions: &HashMap<String, String>,
) -> Result<ScreenshotResult, String> {
    let mut capture_options = options.clone();
    let (annotations, region) = if options.annotate {
        let metrics = client
            .send_command_no_params("Page.getLayoutMetrics", Some(session_id))
            .await?;
        // Resolve a selector only when it actually controls the capture. An
        // explicit clip wins over full-page, which wins over a selector.
        let target = if options.clip.is_none() && !options.full_page {
            if let Some(selector) = options.selector.as_deref() {
                Some(
                    get_rect_for_selector(client, session_id, ref_map, selector, iframe_sessions)
                        .await?
                        .ok_or(
                            "Annotated screenshot: selected element has no readable rectangle",
                        )?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let (region, projection) = annotation_region(options, &metrics, target.as_ref())?;
        let raw = collect_annotations(client, session_id, ref_map, iframe_sessions).await?;
        let raw = filter_annotations(raw, Some(&projection));
        let annotations = project_annotations(&raw, Some(&projection), None);
        capture_options.clip = Some((region.x, region.y, region.width, region.height));
        // Decode lossless pixels for rendering; encode JPEG only once at the
        // requested quality after labels have been drawn.
        capture_options.format = "png".to_string();
        (annotations, Some(region))
    } else {
        (Vec::new(), None)
    };

    // Hide the non-stealth cursor around the raw capture as before.
    set_cursor_visible(client, session_id, false).await;
    let captured = capture_screenshot_base64(
        client,
        session_id,
        ref_map,
        &capture_options,
        iframe_sessions,
    )
    .await;
    set_cursor_visible(client, session_id, true).await;
    let mut base64 = captured?;
    if let Some(region) = region {
        base64 = annotation_image::render(&base64, &annotations, &region, options)?;
    }

    let ext = if options.format == "jpeg" {
        "jpg"
    } else {
        "png"
    };
    let path = save_screenshot(
        &base64,
        options.path.as_deref(),
        ext,
        options.output_dir.as_deref(),
    )?;

    Ok(ScreenshotResult {
        path,
        base64,
        annotations,
    })
}

/// Show/hide the extension's cursor overlay around a capture. No-op on a launched
/// browser or an extension that predates `ABExt.setCursorVisible`.
async fn set_cursor_visible(client: &CdpClient, session_id: &str, visible: bool) {
    if crate::native::interaction::cursor_known_off() || crate::connect::relay_url().is_none() {
        return;
    }
    let _ = client
        .send_command_typed::<_, serde_json::Value>(
            "ABExt.setCursorVisible",
            &serde_json::json!({ "sessionId": session_id, "visible": visible }),
            None,
        )
        .await;
}

async fn capture_screenshot_base64(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    options: &ScreenshotOptions,
    iframe_sessions: &HashMap<String, String>,
) -> Result<String, String> {
    let mut params = CaptureScreenshotParams {
        format: Some(options.format.clone()),
        quality: if options.format == "jpeg" {
            options.quality.or(Some(80))
        } else {
            None
        },
        clip: None,
        from_surface: Some(true),
        capture_beyond_viewport: if options.full_page { Some(true) } else { None },
    };

    if let Some((x, y, width, height)) = options.clip {
        // Explicit pixel region wins over selector/full_page (issue #34).
        // Annotated captures supply a document-space clip even for selectors.
        // Permit off-viewport pixels instead of depending on the compositor's
        // currently visible surface (relay captures can otherwise time out).
        params.capture_beyond_viewport = Some(true);
        params.clip = Some(Viewport {
            x,
            y,
            width,
            height,
            scale: 1.0,
        });
    } else if options.full_page {
        let metrics: Value = client
            .send_command_no_params("Page.getLayoutMetrics", Some(session_id))
            .await?;

        let content_size = metrics
            .get("contentSize")
            .or_else(|| metrics.get("cssContentSize"));
        if let Some(size) = content_size {
            let width = size.get("width").and_then(|v| v.as_f64()).unwrap_or(1280.0);
            let height = size.get("height").and_then(|v| v.as_f64()).unwrap_or(720.0);

            params.clip = Some(Viewport {
                x: 0.0,
                y: 0.0,
                width,
                height,
                scale: 1.0,
            });
        }
    } else if let Some(ref selector) = options.selector {
        if let Some(rect) =
            get_rect_for_selector(client, session_id, ref_map, selector, iframe_sessions).await?
        {
            params.clip = Some(Viewport {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                scale: 1.0,
            });
        }
    }

    // Fill the compositor base with opaque white before capturing. Under
    // `fromSurface: true` (always on), regions the GPU compositor did not paint
    // into the captured surface — common with `position: fixed` / `overflow`
    // scroll / `backdrop-filter` layers under mobile device emulation — come
    // back transparent, which encodes as a solid BLACK region in the output
    // (issue #119). An opaque default background makes those uncaptured regions
    // render white (matching the light page the user actually sees) instead of
    // black. It draws behind all real content, so fully-painted pages are
    // unaffected. Restored immediately after so it never leaks into the live
    // page or later captures.
    let _ = client
        .send_command(
            "Emulation.setDefaultBackgroundColorOverride",
            Some(serde_json::json!({ "color": { "r": 255, "g": 255, "b": 255, "a": 255 } })),
            Some(session_id),
        )
        .await;

    let result: Result<CaptureScreenshotResult, String> = client
        .send_command_typed("Page.captureScreenshot", &params, Some(session_id))
        .await;

    // Clear the override regardless of capture success (empty params = reset).
    let _ = client
        .send_command(
            "Emulation.setDefaultBackgroundColorOverride",
            Some(serde_json::json!({})),
            Some(session_id),
        )
        .await;

    Ok(result?.data)
}

async fn collect_annotations(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    iframe_sessions: &HashMap<String, String>,
) -> Result<Vec<RawAnnotation>, String> {
    let entries = ref_map.entries_sorted();
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    // Protocol box models include same-renderer frame offsets. OOP frames
    // require their own session and a projection through their owner element.
    // A backendNodeId is renderer-local: resolving it in the top session may
    // silently select an unrelated node with the same numeric id.
    let geometry = geometry::FrameGeometry::load(client, session_id, iframe_sessions).await?;
    let mut requests = Vec::new();
    let mut refs = Vec::new();
    for (ref_id, entry) in entries {
        if let Some(backend_node_id) = entry.backend_node_id {
            let effective_session = geometry.session_for(entry.frame_id.as_deref())?;
            requests.push(client.send_command(
                "DOM.getBoxModel",
                Some(serde_json::json!({"backendNodeId":backend_node_id})),
                Some(effective_session),
            ));
            refs.push((ref_id, entry, effective_session));
        }
    }
    let mut annotations = Vec::new();
    for ((ref_id, entry, effective_session), response) in refs
        .into_iter()
        .zip(futures_util::future::join_all(requests).await)
    {
        // Detached/hidden nodes can have no layout box. Do not invent one.
        let Ok(response) = response else { continue };
        let rect = geometry.rect(effective_session, &response["model"]["border"])?;
        if rect.width <= 0.0 || rect.height <= 0.0 {
            continue;
        }
        let number = ref_id
            .strip_prefix('e')
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        annotations.push(RawAnnotation {
            ref_id,
            number,
            role: entry.role,
            name: (!entry.name.is_empty()).then_some(entry.name),
            rect,
        });
    }

    Ok(annotations)
}

async fn get_rect_for_selector(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    selector: &str,
    iframe_sessions: &HashMap<String, String>,
) -> Result<Option<Rect>, String> {
    let (object_id, effective_session_id) = super::element::resolve_element_object_id(
        client,
        session_id,
        ref_map,
        selector,
        iframe_sessions,
    )
    .await?;
    let geometry = geometry::FrameGeometry::load(client, session_id, iframe_sessions).await?;
    let result = client
        .send_command(
            "DOM.getBoxModel",
            Some(serde_json::json!({"objectId":object_id})),
            Some(&effective_session_id),
        )
        .await?;
    geometry
        .rect(&effective_session_id, &result["model"]["border"])
        .map(Some)
}

fn parse_rect(value: &Value) -> Option<Rect> {
    Some(Rect {
        x: value.get("x")?.as_f64()?,
        y: value.get("y")?.as_f64()?,
        width: value.get("width")?.as_f64()?,
        height: value.get("height")?.as_f64()?,
    })
}

fn filter_annotations(
    annotations: Vec<RawAnnotation>,
    target_rect: Option<&Rect>,
) -> Vec<RawAnnotation> {
    let mut items = annotations
        .into_iter()
        .filter(|annotation| match target_rect {
            Some(target) => overlaps(&annotation.rect, target),
            None => true,
        })
        .collect::<Vec<_>>();

    items.sort_by_key(|annotation| annotation.number);
    items
}

fn overlaps(left: &Rect, right: &Rect) -> bool {
    let left_x2 = left.x + left.width;
    let left_y2 = left.y + left.height;
    let right_x2 = right.x + right.width;
    let right_y2 = right.y + right.height;

    left.x < right_x2 && left_x2 > right.x && left.y < right_y2 && left_y2 > right.y
}

/// The document-space capture region and the same region relative to the
/// layout viewport used by getBoundingClientRect. CSS metrics avoid mixing
/// device pixels with DOM rectangles under device emulation.
fn annotation_region(
    options: &ScreenshotOptions,
    metrics: &Value,
    target: Option<&Rect>,
) -> Result<(Rect, Rect), String> {
    let layout = metrics
        .get("cssLayoutViewport")
        .or_else(|| metrics.get("layoutViewport"))
        .ok_or("Annotated screenshot: layout viewport metrics unavailable")?;
    let scroll_x = layout
        .get("pageX")
        .and_then(Value::as_f64)
        .ok_or("Annotated screenshot: horizontal scroll unavailable")?;
    let scroll_y = layout
        .get("pageY")
        .and_then(Value::as_f64)
        .ok_or("Annotated screenshot: vertical scroll unavailable")?;
    let region = if let Some((x, y, width, height)) = options.clip {
        Rect {
            x,
            y,
            width,
            height,
        }
    } else if options.full_page {
        metrics
            .get("cssContentSize")
            .or_else(|| metrics.get("contentSize"))
            .and_then(parse_rect)
            .ok_or("Annotated screenshot: content size unavailable")?
    } else if let Some(target) = target {
        Rect {
            x: target.x + scroll_x,
            y: target.y + scroll_y,
            ..target.clone()
        }
    } else {
        let viewport = metrics
            .get("cssVisualViewport")
            .or_else(|| metrics.get("visualViewport"))
            .unwrap_or(layout);
        Rect {
            x: viewport
                .get("pageX")
                .and_then(Value::as_f64)
                .unwrap_or(scroll_x),
            y: viewport
                .get("pageY")
                .and_then(Value::as_f64)
                .unwrap_or(scroll_y),
            width: viewport
                .get("clientWidth")
                .and_then(Value::as_f64)
                .ok_or("Annotated screenshot: viewport width unavailable")?,
            height: viewport
                .get("clientHeight")
                .and_then(Value::as_f64)
                .ok_or("Annotated screenshot: viewport height unavailable")?,
        }
    };
    if ![
        scroll_x,
        scroll_y,
        region.x,
        region.y,
        region.width,
        region.height,
    ]
    .iter()
    .all(|v| v.is_finite())
        || region.width <= 0.0
        || region.height <= 0.0
    {
        return Err("Annotated screenshot: invalid capture geometry".to_string());
    }
    let projection = Rect {
        x: region.x - scroll_x,
        y: region.y - scroll_y,
        ..region.clone()
    };
    Ok((region, projection))
}

fn project_annotations(
    annotations: &[RawAnnotation],
    target_rect: Option<&Rect>,
    scroll: Option<(f64, f64)>,
) -> Vec<ScreenshotAnnotation> {
    annotations
        .iter()
        .map(|annotation| {
            let rect = if let Some(target) = target_rect {
                Rect {
                    x: annotation.rect.x - target.x,
                    y: annotation.rect.y - target.y,
                    width: annotation.rect.width,
                    height: annotation.rect.height,
                }
            } else if let Some((scroll_x, scroll_y)) = scroll {
                Rect {
                    x: annotation.rect.x + scroll_x,
                    y: annotation.rect.y + scroll_y,
                    width: annotation.rect.width,
                    height: annotation.rect.height,
                }
            } else {
                annotation.rect.clone()
            };

            ScreenshotAnnotation {
                ref_id: annotation.ref_id.clone(),
                number: annotation.number,
                role: annotation.role.clone(),
                name: annotation.name.clone(),
                box_: AnnotationBox {
                    x: round(rect.x),
                    y: round(rect.y),
                    width: round(rect.width),
                    height: round(rect.height),
                },
            }
        })
        .collect()
}

fn save_screenshot(
    base64_data: &str,
    explicit_path: Option<&str>,
    ext: &str,
    output_dir: Option<&str>,
) -> Result<String, String> {
    let save_path = match explicit_path {
        Some(path) => path.to_string(),
        None => {
            let dir = match output_dir {
                Some(d) => PathBuf::from(d),
                None => get_screenshot_dir(),
            };
            let _ = std::fs::create_dir_all(&dir);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let name = format!("screenshot-{}.{}", timestamp, ext);
            dir.join(name).to_string_lossy().to_string()
        }
    };

    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64_data)
        .map_err(|e| format!("Failed to decode screenshot: {}", e))?;

    std::fs::write(&save_path, &bytes)
        .map_err(|e| format!("Failed to save screenshot to {}: {}", save_path, e))?;

    Ok(save_path)
}

fn round(value: f64) -> i64 {
    value.round() as i64
}

fn get_screenshot_dir() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        home.join(".chrome-use").join("tmp").join("screenshots")
    } else {
        std::env::temp_dir().join("chrome-use").join("screenshots")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Value {
        serde_json::json!({
            "cssLayoutViewport": {"pageX":10,"pageY":500,"clientWidth":100,"clientHeight":80},
            "cssVisualViewport": {"pageX":15,"pageY":530,"clientWidth":80,"clientHeight":60},
            "cssContentSize": {"x":0,"y":0,"width":200,"height":2000},
            "contentSize": {"x":0,"y":0,"width":400,"height":4000}
        })
    }

    #[test]
    fn annotation_region_uses_visual_viewport_and_layout_scroll() {
        let (capture, projection) =
            annotation_region(&ScreenshotOptions::default(), &metrics(), None).unwrap();
        assert_eq!(
            (capture.x, capture.y, capture.width, capture.height),
            (15.0, 530.0, 80.0, 60.0)
        );
        assert_eq!((projection.x, projection.y), (5.0, 30.0));
    }

    #[test]
    fn annotation_region_prefers_css_content_size_for_full_page() {
        let options = ScreenshotOptions {
            full_page: true,
            ..Default::default()
        };
        let (capture, projection) = annotation_region(&options, &metrics(), None).unwrap();
        assert_eq!((capture.width, capture.height), (200.0, 2000.0));
        assert_eq!((projection.x, projection.y), (-10.0, -500.0));
    }

    #[test]
    fn annotation_region_translates_scrolled_selector_to_document_space() {
        let target = Rect {
            x: 20.0,
            y: 50.0,
            width: 30.0,
            height: 40.0,
        };
        let (capture, projection) =
            annotation_region(&ScreenshotOptions::default(), &metrics(), Some(&target)).unwrap();
        assert_eq!((capture.x, capture.y), (30.0, 550.0));
        assert_eq!((projection.x, projection.y), (20.0, 50.0));
    }

    #[test]
    fn annotation_region_explicit_clip_wins_over_full_page_and_selector() {
        let options = ScreenshotOptions {
            clip: Some((100.0, 800.0, 50.0, 60.0)),
            full_page: true,
            ..Default::default()
        };
        let target = Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        let (capture, projection) = annotation_region(&options, &metrics(), Some(&target)).unwrap();
        assert_eq!(
            (capture.x, capture.y, capture.width, capture.height),
            (100.0, 800.0, 50.0, 60.0)
        );
        assert_eq!((projection.x, projection.y), (90.0, 300.0));
    }

    #[test]
    fn annotation_region_refuses_missing_or_invalid_geometry() {
        assert!(annotation_region(&ScreenshotOptions::default(), &Value::Null, None).is_err());
        for width in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let options = ScreenshotOptions {
                clip: Some((0.0, 0.0, width, 10.0)),
                ..Default::default()
            };
            assert!(annotation_region(&options, &metrics(), None).is_err());
        }
    }

    #[test]
    fn filters_annotations_to_target_overlap() {
        let annotations = vec![
            RawAnnotation {
                ref_id: "e1".to_string(),
                number: 1,
                role: "button".to_string(),
                name: Some("Inside".to_string()),
                rect: Rect {
                    x: 10.0,
                    y: 10.0,
                    width: 50.0,
                    height: 20.0,
                },
            },
            RawAnnotation {
                ref_id: "e2".to_string(),
                number: 2,
                role: "button".to_string(),
                name: Some("Outside".to_string()),
                rect: Rect {
                    x: 200.0,
                    y: 200.0,
                    width: 40.0,
                    height: 20.0,
                },
            },
        ];

        let target = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };

        let filtered = filter_annotations(annotations, Some(&target));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].ref_id, "e1");
    }

    #[test]
    fn projects_selector_annotations_relative_to_target() {
        let annotations = vec![RawAnnotation {
            ref_id: "e1".to_string(),
            number: 1,
            role: "button".to_string(),
            name: Some("Inside".to_string()),
            rect: Rect {
                x: 25.0,
                y: 35.0,
                width: 40.0,
                height: 20.0,
            },
        }];

        let target = Rect {
            x: 10.0,
            y: 15.0,
            width: 100.0,
            height: 100.0,
        };

        let projected = project_annotations(&annotations, Some(&target), None);
        assert_eq!(projected[0].box_.x, 15);
        assert_eq!(projected[0].box_.y, 20);
    }

    #[test]
    fn projects_full_page_annotations_to_document_space() {
        let annotations = vec![RawAnnotation {
            ref_id: "e1".to_string(),
            number: 1,
            role: "button".to_string(),
            name: Some("Bottom".to_string()),
            rect: Rect {
                x: 5.0,
                y: 12.0,
                width: 40.0,
                height: 20.0,
            },
        }];

        let projected = project_annotations(&annotations, None, Some((10.0, 1000.0)));
        assert_eq!(projected[0].box_.x, 15);
        assert_eq!(projected[0].box_.y, 1012);
    }
}
