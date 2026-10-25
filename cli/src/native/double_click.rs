//! Trusted double-click delivery with target checks and bounded observation.
use super::*;

pub struct DoubleClickOutcome {
    pub point: (f64, f64),
    pub verified: bool,
    pub warning: Option<String>,
}

const RECORD: &str = r#"function() {
  const el = this, doc = el.ownerDocument;
  const events = [];
  const on = e => {
    if (!e.isTrusted || !(e.composedPath ? e.composedPath().includes(el) : el.contains(e.target))) return;
    events.push({type:e.type, detail:e.detail});
  };
  const types = ['click', 'dblclick'];
  for (const type of types) doc.addEventListener(type, on, true);
  let timer = 0;
  const stop = () => { for (const type of types) doc.removeEventListener(type, on, true); clearTimeout(timer); };
  timer = setTimeout(stop, 10000);
  return {events, stop};
}"#;

fn checked_points(reply: &Value) -> Result<Vec<(f64, f64)>, String> {
    if let Some(error) = reply.get("error").and_then(Value::as_str) {
        return Err(format!(
            "dblclick refused: cannot safely target the element ({error})"
        ));
    }
    if reply.get("ancestor").is_some() {
        return Err(
            "dblclick refused: pointer events would reach an ancestor instead of the target".into(),
        );
    }
    let points: Vec<_> = reply
        .get("points")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let x = p.get("gx")?.as_f64()?;
            let y = p.get("gy")?.as_f64()?;
            (x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0).then_some((x, y))
        })
        .collect();
    if points.is_empty() {
        return Err("dblclick refused: no verified top-level hit point".into());
    }
    Ok(points)
}

async fn points(
    client: &CdpClient,
    session: &str,
    object: &str,
) -> Result<Vec<(f64, f64)>, String> {
    let script = format!("function() {{ if (this.matches(':disabled') || this.getAttribute('aria-disabled') === 'true') return {{error:'disabled'}}; return ({HOVER_HIT_POINT_JS}).call(this); }}");
    let result = call_on(client, session, object, &script).await?;
    if result.exception_details.is_some() {
        return Err("dblclick refused: target geometry could not be inspected".into());
    }
    checked_points(&result.result.value.unwrap_or(Value::Null))
}

fn confirmed(events: &Value) -> bool {
    let Some(events) = events.as_array() else {
        return false;
    };
    events.len() == 3
        && events[0]["type"] == "click"
        && events[0]["detail"] == 1
        && events[1]["type"] == "click"
        && events[1]["detail"] == 2
        && events[2]["type"] == "dblclick"
        && events[2]["detail"] == 2
}

/// Deliver two native press/release pairs to a verified top-level point. Frame
/// geometry that cannot be mapped safely is refused rather than DOM-dispatched.
/// Verification means trusted double-click delivery, not application success.
pub async fn dblclick(
    client: &std::sync::Arc<CdpClient>,
    session_id: &str,
    ref_map: &RefMap,
    selector_or_ref: &str,
    iframe_sessions: &HashMap<String, String>,
) -> Result<DoubleClickOutcome, String> {
    if std::env::var("AGENT_BROWSER_CLICK_MODE").as_deref() == Ok("dom") {
        return Err("dblclick requires trusted pointer input; AGENT_BROWSER_CLICK_MODE=dom is not supported for this command".into());
    }
    let (object, element_session) = resolve_element_object_id(
        client,
        session_id,
        ref_map,
        selector_or_ref,
        iframe_sessions,
    )
    .await?;
    deliver(client, session_id, &element_session, &object).await
}

async fn deliver(
    client: &std::sync::Arc<CdpClient>,
    session_id: &str,
    element_session: &str,
    object: &str,
) -> Result<DoubleClickOutcome, String> {
    let candidates = points(client, element_session, object).await?;
    let point = candidates[humanize::next_seed() as usize % candidates.len()];
    let installed = call_on_record(client, element_session, object).await?;
    let guard = RecorderGuard {
        client: client.clone(),
        session: element_session.to_string(),
        object_id: Some(installed.clone()),
    };
    move_pointer(client, session_id, point.0, point.1).await?;
    for count in 1..=2 {
        // Recheck after pointer movement and after the first click: handlers may
        // move, cover, disable or remove the target. Never guess the second hit.
        let current = points(client, element_session, object).await.map_err(|e| {
            format!("{e}; before click {count} of 2, earlier input may have taken effect")
        })?;
        if !current
            .iter()
            .any(|p| (p.0 - point.0).abs() < 0.5 && (p.1 - point.1).abs() < 0.5)
        {
            return Err(format!("dblclick stopped before click {count} of 2: target geometry changed; inspect the page before retrying"));
        }
        for (event_type, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            client.send_command("Input.dispatchMouseEvent", Some(json!({
                "type":event_type,"x":point.0,"y":point.1,"button":"left","buttons":buttons,"clickCount":count
            })),Some(session_id)).await.map_err(|e| {
                format!("dblclick interrupted during click {count} of 2: {e}; input may have taken effect, inspect before retrying")
            })?;
            if event_type == "mousePressed" {
                tokio::time::sleep(humanize::press_dwell(
                    humanize::active_level(),
                    humanize::next_seed(),
                ))
                .await;
            }
        }
        if count == 1 {
            tokio::time::sleep(humanize::press_dwell(
                humanize::active_level(),
                humanize::next_seed(),
            ))
            .await;
        }
    }
    wait_for_paint_settled(client, session_id).await;
    let events = call_on(
        client,
        element_session,
        &installed,
        "function() { return this.events; }",
    )
    .await
    .ok()
    .filter(|r| r.exception_details.is_none())
    .and_then(|r| r.result.value)
    .unwrap_or(Value::Null);
    guard.finish().await;
    let verified = confirmed(&events);
    Ok(DoubleClickOutcome {
        point, verified,
        warning: (!verified).then(|| "double-click input was sent, but the target's trusted click/click/dblclick sequence could not be confirmed; inspect the result before retrying".into()),
    })
}

async fn call_on_record(client: &CdpClient, session: &str, object: &str) -> Result<String, String> {
    let result = client
        .send_command(
            "Runtime.callFunctionOn",
            Some(json!({
                "objectId":object,"functionDeclaration":RECORD,"returnByValue":false,
            })),
            Some(session),
        )
        .await?;
    if result.get("exceptionDetails").is_some() {
        return Err("dblclick refused: event verification could not be installed".into());
    }
    result
        .pointer("/result/objectId")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| "dblclick refused: event verification handle unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unmappable_covered_disabled_or_ancestor_targets() {
        for error in [
            "cross-origin-frame",
            "transformed-frame",
            "covered",
            "disabled",
            "detached",
        ] {
            assert!(checked_points(&json!({"error":error})).is_err());
        }
        assert!(checked_points(&json!({"ancestor":"div","points":[{"gx":1,"gy":2}]})).is_err());
        assert!(checked_points(&json!({"points":[{"gx":-1,"gy":2}]})).is_err());
        assert_eq!(
            checked_points(&json!({"points":[{"gx":1,"gy":2}]})).unwrap(),
            [(1.0, 2.0)]
        );
    }
    #[test]
    fn confirms_only_the_complete_double_click_sequence() {
        let expected = json!([{"type":"click","detail":1},{"type":"click","detail":2},{"type":"dblclick","detail":2}]);
        assert!(confirmed(&expected));
        for value in [
            Value::Null,
            json!([]),
            json!([{"type":"dblclick","detail":2}]),
            json!([{"type":"click","detail":1},{"type":"click","detail":1}]),
        ] {
            assert!(!confirmed(&value));
        }
    }
}

#[cfg(test)]
#[path = "double_click_tests.rs"]
mod protocol_tests;
