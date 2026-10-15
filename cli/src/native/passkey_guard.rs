//! The `create()` guard of `auth login --bwu`: while chrome-use's temporary
//! virtual authenticator is installed, the page cannot register a passkey
//! with it. Every `navigator.credentials.create()` is refused before it
//! reaches Chrome, with the error Chrome itself gives for a refused ceremony.
//! A passkey sign-in has no business creating any credential, so nothing is
//! passed through and the original is never called from the page's realm.
//! Chrome's `credentialAdded` event stays the backstop.
//!
//! The guard is a `Proxy` around the original on `CredentialsContainer`'s
//! prototype (so `CredentialsContainer.prototype.create.call(...)` is refused
//! too), and around an own `create` of the `navigator.credentials` instance
//! when the page put one there. Its handler has no prototype, so no trap
//! lookup ever reaches page code (`Object.prototype.get` and friends would
//! otherwise be called with the handler and the original). Page script has no
//! way to read a proxy's target or handler; only CDP can, through the proxy's
//! `[[Target]]`. The page can still tell a guard is there:
//! `Function.prototype.toString` gives `function () { [native code] }`,
//! without the native's name.
//!
//! Where it is installed:
//! - every new document of the tab and of each attached out-of-process frame,
//!   by `Page.addScriptToEvaluateOnNewDocument`, which runs before any page
//!   script; the builtins it uses are taken before anything else runs;
//! - right away, in the current document and in every same-process frame
//!   (reached in its main world through its document node, without
//!   `Runtime.enable`) and in each attached out-of-process frame.
//!
//! Known limits, by design: a document that ran before the guard (the page
//! open when the login starts, and its frames) can have kept the original
//! `create` or replaced `Proxy` / `Object.defineProperty` first; page script
//! cannot win against page script that ran first. An out-of-process frame
//! that attaches after the install is not guarded. In both cases a
//! registration still shows up as `credentialAdded` and stops the login.
//!
//! Restoring is by identity, never by source text: a guard installed through
//! CDP is the very object whose handle we kept; one the new-document script
//! made carries this install's random key on its handler, which the page can
//! neither read nor guess. A proxy that only looks like ours is left alone.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use super::cdp::client::CdpClient;
use super::element::GroupRelease;

/// Object group of every handle the guard holds or uses.
pub(super) const GROUP: &str = "chrome-use-passkey-guard";

/// The guard installer, as a function of the handler key (`null` for a
/// guard installed through CDP, which is known by its handle). Returns
/// `[prototype, instance, prototype guard, instance guard or null]`, or null
/// when `create` could not be guarded.
const INSTALLER: &str = r#"function (key) {
    'use strict';
    const P = Proxy, O = Object, proto = O.getPrototypeOf, own = O.getOwnPropertyDescriptor, define = O.defineProperty;
    const reject = Promise.reject.bind(Promise), E = DOMException;
    const msg = 'The operation either timed out or was not allowed. See: https://www.w3.org/TR/webauthn-2/#sctn-privacy-considerations-client.';
    const c = navigator.credentials;
    const p = c && proto(c);
    const d = p && own(p, 'create');
    if (!d || typeof d.value !== 'function') return null;
    const o = own(c, 'create');
    if (o && typeof o.value !== 'function') return null;
    const wrap = (f) => {
        const h = { __proto__: null, apply() { return reject(new E(msg, 'NotAllowedError')); } };
        if (key) h[key] = 0;
        return new P(f, h);
    };
    const gp = wrap(d.value), gc = o ? wrap(o.value) : null;
    try {
        define(p, 'create', { __proto__: null, ...d, value: gp });
        if (o) define(c, 'create', { __proto__: null, ...o, value: gc });
    } catch (e) {
        return null;
    }
    // Only when the guards really replaced create().
    if (own(p, 'create').value !== gp || (o && own(c, 'create').value !== gc)) return null;
    return [p, c, gp, gc];
}"#;

/// The new-document script: the installer, keyed.
pub(super) fn no_create_js(key: &str) -> String {
    format!("({INSTALLER})({});", Value::String(key.to_string()))
}

/// A fresh key for one install. Only the new-document script carries it.
pub(super) fn new_key() -> String {
    format!("k{}", uuid::Uuid::new_v4().simple())
}

/// Guards installed through CDP in one document.
struct Installed {
    session: String,
    frame: String,
    /// `CredentialsContainer.prototype` and `navigator.credentials`.
    holders: [String; 2],
    /// The proxies on them (the instance one only when it had an own create).
    guards: [Option<String>; 2],
}

/// Everything one install did, for [`restore`].
pub(super) struct CreateGuard {
    key: String,
    /// (session, new-document script identifier).
    scripts: Vec<(String, String)>,
    installed: Vec<Installed>,
    /// Releases [`GROUP`] in each session holding handles, also when the
    /// login is cancelled.
    holds: Vec<GroupRelease>,
}

impl CreateGuard {
    pub(super) fn new() -> Self {
        Self {
            key: new_key(),
            scripts: Vec::new(),
            installed: Vec::new(),
            holds: Vec::new(),
        }
    }

    fn hold(&mut self, client: &Arc<CdpClient>, session: &str) {
        if !self.holds.iter().any(|h| h.session() == session) {
            self.holds
                .push(GroupRelease::new(client.clone(), session, GROUP));
        }
    }
}

/// An error that only says the frame, document or session is gone.
pub(super) fn gone(e: &str) -> bool {
    [
        "Cannot find context with specified id",
        "Execution context was destroyed",
        "Inspected target navigated or closed",
        "Could not find object with given id",
        "No session with given id",
        "No frame with given id",
        "Frame with the given id was not found",
        "No node with given id",
        "Node with given id does not belong to the document",
        "Session with given id not found",
    ]
    .iter()
    .any(|m| e.contains(m))
}

fn threw(v: &Value) -> Option<String> {
    v.get("exceptionDetails")
        .map(|e| format!("page script threw: {}", e["text"].as_str().unwrap_or("?")))
}

fn object_id(v: &Value) -> Option<String> {
    v["result"]["objectId"].as_str().map(str::to_string)
}

/// The page session first, then each attached out-of-process frame's.
fn sessions(page: &str, iframes: &HashMap<String, String>, extra: &[&str]) -> Vec<String> {
    let mut out = vec![page.to_string()];
    let mut rest: Vec<&String> = iframes.values().collect();
    rest.sort();
    for s in rest
        .into_iter()
        .map(String::as_str)
        .chain(extra.iter().copied())
    {
        if !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    out
}

/// The frames whose documents live in `session`: its root, then the
/// same-process children (out-of-process ones are their own sessions).
async fn frames(client: &CdpClient, session: &str) -> Result<Vec<String>, String> {
    let tree = client
        .send_command_no_params("Page.getFrameTree", Some(session))
        .await?;
    let mut all = Vec::new();
    super::element::flatten_frame_tree(&tree["frameTree"], true, &mut all);
    Ok(all.into_iter().map(|(id, _, _)| id).collect())
}

/// Run `function` in the main world of `frame` (`root`: the session's own
/// document; otherwise through the frame's document node). Ok(None) when the
/// frame has no document in this session.
async fn call_in_frame(
    client: &CdpClient,
    session: &str,
    frame: &str,
    root: bool,
    function: &str,
    args: Value,
) -> Result<Option<Value>, String> {
    let r = if root {
        client
            .send_command(
                "Runtime.evaluate",
                Some(json!({
                    "expression": format!(
                        "({function})({})",
                        args.as_array()
                            .map(|a| a.iter().map(Value::to_string).collect::<Vec<_>>().join(", "))
                            .unwrap_or_default()
                    ),
                    "objectGroup": GROUP,
                })),
                Some(session),
            )
            .await?
    } else {
        let owner = client
            .send_command(
                "DOM.getFrameOwner",
                Some(json!({ "frameId": frame })),
                Some(session),
            )
            .await?;
        let Some(owner) = owner["backendNodeId"].as_i64() else {
            return Ok(None);
        };
        let node = client
            .send_command(
                "DOM.describeNode",
                Some(json!({ "backendNodeId": owner, "depth": 1 })),
                Some(session),
            )
            .await?;
        let Some(doc) = node
            .pointer("/node/contentDocument/backendNodeId")
            .and_then(Value::as_i64)
        else {
            return Ok(None);
        };
        // No execution context named: the document's main world.
        let doc = client
            .send_command(
                "DOM.resolveNode",
                Some(json!({ "backendNodeId": doc, "objectGroup": GROUP })),
                Some(session),
            )
            .await?;
        let Some(doc) = doc.pointer("/object/objectId").and_then(Value::as_str) else {
            return Ok(None);
        };
        let args: Vec<Value> = args
            .as_array()
            .map(|a| a.iter().map(|v| json!({ "value": v })).collect())
            .unwrap_or_default();
        client
            .send_command(
                "Runtime.callFunctionOn",
                Some(json!({
                    "objectId": doc,
                    "functionDeclaration": function,
                    "arguments": args,
                    "objectGroup": GROUP,
                })),
                Some(session),
            )
            .await?
    };
    if let Some(e) = threw(&r) {
        return Err(e);
    }
    Ok(Some(r))
}

/// The elements of a remote array, by index (`None` for null ones).
async fn elements(
    client: &CdpClient,
    session: &str,
    array: &str,
    n: usize,
) -> Result<Vec<Option<String>>, String> {
    let props = client
        .send_command(
            "Runtime.getProperties",
            Some(json!({ "objectId": array, "ownProperties": true })),
            Some(session),
        )
        .await?;
    Ok((0..n)
        .map(|i| {
            props["result"].as_array().and_then(|a| {
                a.iter()
                    .find(|p| p["name"].as_str().and_then(|n| n.parse::<usize>().ok()) == Some(i))
                    .and_then(|p| p["value"]["objectId"].as_str().map(str::to_string))
            })
        })
        .collect())
}

/// Guard every document of the tab now and every new one from here on.
/// On Err the caller still [`restore`]s, which undoes what was done.
pub(super) async fn install(
    client: &Arc<CdpClient>,
    page_session: &str,
    iframe_sessions: &HashMap<String, String>,
    guard: &mut CreateGuard,
) -> Result<(), String> {
    let source = no_create_js(&guard.key);
    for session in sessions(page_session, iframe_sessions, &[]) {
        let page = session == page_session;
        match client
            .send_command(
                "Page.addScriptToEvaluateOnNewDocument",
                Some(json!({ "source": source })),
                Some(&session),
            )
            .await
        {
            Ok(v) => match v["identifier"].as_str() {
                Some(id) => guard.scripts.push((session.clone(), id.to_string())),
                None => return Err("no script identifier".to_string()),
            },
            // An out-of-process frame that went away needs no guard.
            Err(e) if !page && gone(&e) => continue,
            Err(e) => return Err(e),
        }
        guard.hold(client, &session);
        let list = match frames(client, &session).await {
            Ok(l) => l,
            Err(e) if !page && gone(&e) => continue,
            Err(e) => return Err(e),
        };
        for (i, frame) in list.iter().enumerate() {
            let root = i == 0;
            let r = match call_in_frame(client, &session, frame, root, INSTALLER, json!([null]))
                .await
            {
                Ok(Some(r)) => r,
                // Out of process (its own session), or gone.
                Ok(None) => continue,
                Err(e) if gone(&e) && !(page && root) => continue,
                Err(e) => return Err(format!("frame {frame}: {e}")),
            };
            let Some(array) = object_id(&r) else {
                return Err(format!("frame {frame}: create() could not be guarded"));
            };
            let e = elements(client, &session, &array, 4).await?;
            let (Some(p), Some(c), Some(gp)) = (e[0].clone(), e[1].clone(), e[2].clone()) else {
                return Err(format!("frame {frame}: create() could not be guarded"));
            };
            guard.installed.push(Installed {
                session: session.clone(),
                frame: frame.clone(),
                holders: [p, c],
                guards: [Some(gp), e[3].clone()],
            });
        }
    }
    Ok(())
}

/// What restoring one document found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    /// A guard of ours was taken off.
    Restored,
    /// Nothing of ours was there.
    Clean,
    /// The document went away, and its guard with it.
    Gone,
    /// A guard of ours is still in effect.
    Failed(String),
}

/// One line per document restore looked at: (frame id, outcome).
pub(super) type RestoreReport = Vec<(String, Outcome)>;

/// Ok when no guard of ours is left anywhere; Err names each frame where one
/// is.
pub(super) fn report_result(report: &RestoreReport) -> Result<(), String> {
    let failed: Vec<String> = report
        .iter()
        .filter_map(|(f, r)| match r {
            Outcome::Failed(e) => Some(format!("frame {f}: {e}")),
            _ => None,
        })
        .collect();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// Undo [`install`] through CDP only: remove the new-document scripts, then
/// take off every guard of ours in the documents we guarded and in every
/// document there is now (the script guarded the ones created since), putting
/// back each proxy's `[[Target]]`. [`GROUP`] is released in every session
/// touched, also if this is cancelled.
pub(super) async fn restore(
    client: &Arc<CdpClient>,
    page_session: &str,
    iframe_sessions: &HashMap<String, String>,
    guard: &mut CreateGuard,
) -> RestoreReport {
    let mut report = RestoreReport::new();
    let script_sessions: Vec<String> = guard.scripts.iter().map(|(s, _)| s.clone()).collect();
    let all = sessions(
        page_session,
        iframe_sessions,
        &script_sessions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    for s in &all {
        guard.hold(client, s);
    }
    for (session, id) in std::mem::take(&mut guard.scripts) {
        if let Err(e) = client
            .send_command(
                "Page.removeScriptToEvaluateOnNewDocument",
                Some(json!({ "identifier": id })),
                Some(&session),
            )
            .await
        {
            if !gone(&e) {
                report.push((
                    session.clone(),
                    Outcome::Failed(format!("removing the new-document guard script: {e}")),
                ));
            }
        }
    }
    // The documents guarded through CDP, by the handles kept.
    for i in &guard.installed {
        let mut out = Outcome::Clean;
        for (holder, mine) in i.holders.iter().zip(&i.guards) {
            let Some(mine) = mine else { continue };
            match restore_slot(client, &i.session, holder, &[mine.as_str()], &guard.key).await {
                Ok(true) => out = Outcome::Restored,
                Ok(false) => {}
                Err(e) if gone(&e) => {
                    out = Outcome::Gone;
                    break;
                }
                Err(e) => {
                    out = Outcome::Failed(e);
                    break;
                }
            }
        }
        report.push((i.frame.clone(), out));
    }
    // Every document there is now, for guards the script made.
    for session in &all {
        let list = match frames(client, session).await {
            Ok(l) => l,
            Err(e) if gone(&e) => continue,
            Err(e) => {
                report.push((session.clone(), Outcome::Failed(e)));
                continue;
            }
        };
        for (n, frame) in list.iter().enumerate() {
            let out = sweep_frame(client, session, frame, n == 0, &guard.key).await;
            match report.iter_mut().find(|(f, _)| f == frame) {
                Some(seen) => seen.1 = merge(std::mem::replace(&mut seen.1, Outcome::Clean), out),
                None => report.push((frame.clone(), out)),
            }
        }
    }
    for h in std::mem::take(&mut guard.holds) {
        h.release().await;
    }
    guard.installed.clear();
    report
}

/// Two looks at one frame (its guarded document by handle, then whatever
/// document it has now): a guard left anywhere wins, then one taken off.
fn merge(a: Outcome, b: Outcome) -> Outcome {
    match (a, b) {
        (f @ Outcome::Failed(_), _) | (_, f @ Outcome::Failed(_)) => f,
        (Outcome::Restored, _) | (_, Outcome::Restored) => Outcome::Restored,
        (a, _) => a,
    }
}

async fn sweep_frame(
    client: &CdpClient,
    session: &str,
    frame: &str,
    root: bool,
    key: &str,
) -> Outcome {
    const HOLDERS: &str = "function () { const c = navigator.credentials; return c ? [Object.getPrototypeOf(c), c] : null; }";
    let r = match call_in_frame(client, session, frame, root, HOLDERS, json!([])).await {
        Ok(Some(r)) => r,
        Ok(None) => return Outcome::Clean,
        Err(e) if gone(&e) => return Outcome::Gone,
        Err(e) => return Outcome::Failed(e),
    };
    let Some(array) = object_id(&r) else {
        return Outcome::Clean;
    };
    let holders = match elements(client, session, &array, 2).await {
        Ok(h) => h,
        Err(e) if gone(&e) => return Outcome::Gone,
        Err(e) => return Outcome::Failed(e),
    };
    let mut out = Outcome::Clean;
    for holder in holders.into_iter().flatten() {
        match restore_slot(client, session, &holder, &[], key).await {
            Ok(true) => out = Outcome::Restored,
            Ok(false) => {}
            Err(e) if gone(&e) => return Outcome::Gone,
            Err(e) => return Outcome::Failed(e),
        }
    }
    out
}

/// The current own data value of `create` on `holder`, read through CDP
/// (no getter, no page code).
async fn own_create(
    client: &CdpClient,
    session: &str,
    holder: &str,
) -> Result<Option<String>, String> {
    let props = client
        .send_command(
            "Runtime.getProperties",
            Some(json!({ "objectId": holder, "ownProperties": true })),
            Some(session),
        )
        .await?;
    Ok(props["result"].as_array().and_then(|a| {
        a.iter()
            .find(|p| p["name"] == "create")
            .and_then(|p| p["value"]["objectId"].as_str().map(str::to_string))
    }))
}

/// `[[Target]]` and `[[Handler]]` of `object` when it is a proxy.
async fn proxy_parts(
    client: &CdpClient,
    session: &str,
    object: &str,
) -> Result<Option<(String, String)>, String> {
    let props = client
        .send_command(
            "Runtime.getProperties",
            Some(json!({ "objectId": object, "ownProperties": true })),
            Some(session),
        )
        .await?;
    let internal = |name: &str| {
        props["internalProperties"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["name"] == name))
            .and_then(|p| p["value"]["objectId"].as_str().map(str::to_string))
    };
    Ok(internal("[[Target]]").zip(internal("[[Handler]]")))
}

/// Whether two handles name the same object (`===`, no page code).
async fn same(client: &CdpClient, session: &str, a: &str, b: &str) -> Result<bool, String> {
    let r = client
        .send_command(
            "Runtime.callFunctionOn",
            Some(json!({
                "objectId": a,
                "functionDeclaration": "function (v) { return this === v; }",
                "arguments": [{ "objectId": b }],
                "returnByValue": true,
            })),
            Some(session),
        )
        .await?;
    Ok(r["result"]["value"] == json!(true))
}

/// Whether the proxy with `handler` is ours: one of `mine` (same document),
/// or its handler carries this install's key.
async fn ours(
    client: &CdpClient,
    session: &str,
    object: &str,
    handler: &str,
    mine: &[&str],
    key: &str,
) -> Result<bool, String> {
    for m in mine {
        if same(client, session, m, object).await? {
            return Ok(true);
        }
    }
    let props = client
        .send_command(
            "Runtime.getProperties",
            Some(json!({ "objectId": handler, "ownProperties": true })),
            Some(session),
        )
        .await?;
    Ok(props["result"]
        .as_array()
        .is_some_and(|a| a.iter().any(|p| p["name"] == key)))
}

/// Take our guards off `holder.create`. Ok(true) when one was taken off,
/// Ok(false) when none of ours is there; Err when one still is (under a
/// wrapper the page put on top, or the page kept it).
async fn restore_slot(
    client: &CdpClient,
    session: &str,
    holder: &str,
    mine: &[&str],
    key: &str,
) -> Result<bool, String> {
    let Some(top) = own_create(client, session, holder).await? else {
        return Ok(false);
    };
    // Unwrap every guard of ours on top: a navigation between registering the
    // new-document script and the install on the current page wraps twice.
    let mut current = top;
    let mut unwrapped = false;
    let mut depth = 0;
    while let Some((target, handler)) = proxy_parts(client, session, &current).await? {
        if depth == 8 {
            break;
        }
        depth += 1;
        if !ours(client, session, &current, &handler, mine, key).await? {
            break;
        }
        current = target;
        unwrapped = true;
    }
    if !unwrapped {
        // Not ours on top; one of ours under a wrapper of the page's is
        // still in effect and not ours to unwrap.
        let mut below = current;
        for _ in 0..8 {
            let Some((target, handler)) = proxy_parts(client, session, &below).await? else {
                break;
            };
            if ours(client, session, &below, &handler, mine, key).await? {
                return Err(
                    "the page wrapped the create() guard in its own function; reload the page \
                     to get the original back"
                        .to_string(),
                );
            }
            below = target;
        }
        return Ok(false);
    }
    let put = client
        .send_command(
            "Runtime.callFunctionOn",
            Some(json!({
                "objectId": holder,
                "functionDeclaration": "function (original) { const d = Object.getOwnPropertyDescriptor(this, 'create'); try { Object.defineProperty(this, 'create', { __proto__: null, ...d, value: original }); } catch (e) {} }",
                "arguments": [{ "objectId": current }],
            })),
            Some(session),
        )
        .await?;
    if let Some(e) = threw(&put) {
        return Err(e);
    }
    match own_create(client, session, holder).await? {
        Some(now) if same(client, session, &current, &now).await? => Ok(true),
        _ => Err("the page kept the create() guard".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the real guard against a stub `navigator.credentials` in node (V8,
    /// like Chrome); skipped where node is missing. `setup` runs before the
    /// guard (the page's own state), `probe` after it and prints JSON.
    fn run_node(setup: &str, key: &str, probe: &str) -> Option<Value> {
        let dir = tempfile::tempdir().ok()?;
        let script = dir.path().join("guard.js");
        std::fs::write(
            &script,
            format!(
                r#"'use strict';
let calls = 0;
// A method, like the native: no [[Construct]], so `new` on the guard throws.
const original = {{ create() {{ calls++; return 'ORIGINAL'; }} }}.create;
class CredentialsContainer {{}}
Object.defineProperty(CredentialsContainer.prototype, 'create', {{
    value: original, writable: true, enumerable: true, configurable: true,
}});
Object.defineProperty(globalThis, 'navigator', {{ value: {{ credentials: new CredentialsContainer() }}, configurable: true }});
{setup}
const installed = {guard};
{probe}
"#,
                guard = no_create_js(key),
            ),
        )
        .ok()?;
        let Ok(out) = std::process::Command::new("node").arg(&script).output() else {
            eprintln!("node not found; skipping");
            return None;
        };
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(serde_json::from_slice(&out.stdout).unwrap())
    }

    /// Round 6: the page has no way back to `create`: not through a token
    /// in the source, odd arguments, patched call/apply/Reflect.apply, nor
    /// (round 7) a trap it puts on `Object.prototype`, which a handler with a
    /// prototype would look up and call with the handler and the original.
    #[test]
    fn the_page_has_no_way_back_to_create() {
        let probe = r#"
const c = navigator.credentials, p = CredentialsContainer.prototype;
const src = Function.prototype.toString.call(c.create);
const leaks = [];
const refusals = [];
const traps = ['get', 'set', 'has', 'ownKeys', 'getOwnPropertyDescriptor', 'defineProperty',
    'deleteProperty', 'getPrototypeOf', 'setPrototypeOf', 'isExtensible', 'preventExtensions', 'construct'];
for (const t of traps) {
    Object.defineProperty(Object.prototype, t, { __proto__: null, configurable: true, value: function (...a) {
        if (a.includes(original) || this === undefined || Object.getPrototypeOf(this) === null) leaks.push('trap ' + t);
        return Reflect[t](...a);
    } });
}
const g = c.create;
try { g.foo; g.bar = 1; 'x' in g; Object.keys(g); Object.getOwnPropertyDescriptor(g, 'name');
      Object.defineProperty(g, 'q', { __proto__: null, value: 1, configurable: true }); delete g.q;
      Object.getPrototypeOf(g); Object.isExtensible(g); } catch (e) {}
try { new g(); } catch (e) {}
for (const t of traps) delete Object.prototype[t];
const tokens = src.match(/[0-9a-f]{16,}/g) || [];
const saved = [Function.prototype.call, Function.prototype.apply, Reflect.apply];
Function.prototype.call = function () { if (this === original) leaks.push('call'); };
Function.prototype.apply = function () { if (this === original) leaks.push('apply'); };
Reflect.apply = function (f) { if (f === original) leaks.push('Reflect.apply'); };
const args = [undefined, null, {}, src, original, { publicKey: {} }, ...tokens];
const pending = [];
for (const a of args) {
    for (const call of [() => c.create(a), () => p.create(a), () => new Function('c', 'a', 'return c.create(a)')(c, a)]) {
        let r;
        try { r = call(); } catch (e) { refusals.push('threw ' + e.name); continue; }
        if (r === original || typeof r === 'function') leaks.push('returned a function');
        pending.push(Promise.resolve(r).then(() => leaks.push('resolved'), (e) => refusals.push(e.name)));
    }
}
Promise.all(pending).then(() => {
    [Function.prototype.call, Function.prototype.apply, Reflect.apply] = saved;
    process.stdout.write(JSON.stringify({
        installed: Array.isArray(installed) && installed[2] === p.create && installed[3] === null,
        src, tokens, leaks, calls,
        refusals: [...new Set(refusals)],
        name: c.create.name, length: c.create.length,
        own: Object.prototype.hasOwnProperty.call(c, 'create'),
        enumerable: Object.getOwnPropertyDescriptor(p, 'create').enumerable,
    }));
});
"#;
        let Some(r) = run_node("", &new_key(), probe) else {
            return;
        };
        assert_eq!(r["installed"], true, "{r}");
        assert_eq!(r["src"], "function () { [native code] }", "{r}");
        assert_eq!(r["tokens"], json!([]), "{r}");
        assert_eq!(r["leaks"], json!([]), "{r}");
        assert_eq!(r["calls"], 0, "{r}");
        assert_eq!(r["refusals"], json!(["NotAllowedError"]), "{r}");
        assert_eq!(r["name"], "create", "{r}");
        assert_eq!(r["length"], 0, "{r}");
        // No own property where the page had none: nothing to fingerprint.
        assert_eq!(r["own"], false, "{r}");
        assert_eq!(r["enumerable"], true, "{r}");
    }

    /// Round 7: an own `create` on the instance shadows the prototype; it is
    /// guarded too, and keeps its own descriptor.
    #[test]
    fn an_own_create_on_the_instance_is_guarded_too() {
        let setup = r#"
let ownCalls = 0;
const ownCreate = function create() { ownCalls++; return 'OWN'; };
Object.defineProperty(navigator.credentials, 'create', { value: ownCreate, writable: true, enumerable: false, configurable: true });
"#;
        let probe = r#"
const c = navigator.credentials;
Promise.allSettled([c.create({}), CredentialsContainer.prototype.create.call(c, {})]).then((rs) => {
    const d = Object.getOwnPropertyDescriptor(c, 'create');
    process.stdout.write(JSON.stringify({
        installed: Array.isArray(installed) && installed[3] === d.value,
        outcomes: rs.map((r) => r.status + ':' + (r.reason ? r.reason.name : r.value)),
        calls, ownCalls, enumerable: d.enumerable,
        src: Function.prototype.toString.call(c.create),
    }));
});
"#;
        let Some(r) = run_node(setup, &new_key(), probe) else {
            return;
        };
        assert_eq!(r["installed"], true, "{r}");
        assert_eq!(
            r["outcomes"],
            json!(["rejected:NotAllowedError", "rejected:NotAllowedError"]),
            "{r}"
        );
        assert_eq!(r["calls"], 0, "{r}");
        assert_eq!(r["ownCalls"], 0, "{r}");
        assert_eq!(r["enumerable"], false, "{r}");
        assert_eq!(r["src"], "function () { [native code] }", "{r}");
    }

    /// Round 7: the builtins are taken before anything else runs, and an own
    /// `create` that is not a function, or a getter, fails closed.
    #[test]
    fn an_unguardable_create_fails_closed() {
        let probe = "process.stdout.write(JSON.stringify({ installed }));";
        for setup in [
            "Object.defineProperty(navigator.credentials, 'create', { get() { return original; }, configurable: true });",
            "Object.defineProperty(navigator.credentials, 'create', { value: 1, configurable: true });",
            "Object.defineProperty(navigator.credentials, 'create', { value: function create() {}, writable: false, configurable: false });",
        ] {
            let Some(r) = run_node(setup, &new_key(), probe) else {
                return;
            };
            assert_eq!(r["installed"], Value::Null, "{setup}: {r}");
        }
    }

    /// Round 7: what tells a guard of ours from the page's is the key on its
    /// handler (or our handle), never its source. The key is on our handler
    /// only; a proxy rebuilt from the same trap text has none, and the key
    /// appears nowhere the page can read.
    #[test]
    fn only_our_handler_carries_the_key() {
        let key = new_key();
        let probe = format!(
            r#"
const util = require('util');
const handlerOf = (f) => util.inspect(f, {{ showProxy: true, depth: 4 }});
const ours = handlerOf(navigator.credentials.create);
const forged = new Proxy(function () {{}}, {{ __proto__: null, apply() {{ return reject(new E(msg, 'NotAllowedError')); }} }});
const visible = [Function.prototype.toString.call(navigator.credentials.create),
    Object.getOwnPropertyNames(navigator.credentials.create).join(','),
    JSON.stringify(Object.getOwnPropertyDescriptors(CredentialsContainer.prototype))].join('|');
process.stdout.write(JSON.stringify({{
    ours: ours.includes({key:?}),
    forged: handlerOf(forged).includes({key:?}),
    visible: visible.includes({key:?}),
}}));
"#
        );
        let Some(r) = run_node("", &key, &probe) else {
            return;
        };
        assert_eq!(
            r,
            json!({ "ours": true, "forged": false, "visible": false })
        );
        assert_ne!(new_key(), key);
    }

    async fn recorder() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use futures_util::{SinkExt, StreamExt};
        let released = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = released.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    while let Some(Ok(msg)) = ws.next().await {
                        let tokio_tungstenite::tungstenite::Message::Text(text) = msg else {
                            continue;
                        };
                        let req: Value = serde_json::from_str(&text).unwrap();
                        if req["method"] == "Runtime.releaseObjectGroup" {
                            seen.lock().unwrap().push(format!(
                                "{}:{}",
                                req["sessionId"].as_str().unwrap_or(""),
                                req["params"]["objectGroup"].as_str().unwrap_or("")
                            ));
                        }
                        let mut out = json!({ "id": req["id"], "result": {} });
                        if let Some(sid) = req.get("sessionId") {
                            out["sessionId"] = sid.clone();
                        }
                        let reply = tokio_tungstenite::tungstenite::Message::Text(out.to_string());
                        if ws.send(reply).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (url, released)
    }

    /// Round 7: a restore cancelled part-way (here before its first command
    /// is on the wire) still releases the guard's object group in every
    /// session it touched.
    #[tokio::test]
    async fn restore_releases_the_group_when_cancelled() {
        let (url, released) = recorder().await;
        let client = Arc::new(CdpClient::connect(&url).await.unwrap());
        let mut iframes = HashMap::new();
        iframes.insert("F1".to_string(), "S2".to_string());
        let mut guard = CreateGuard::new();
        guard.scripts.push(("S1".to_string(), "1".to_string()));
        let writer = client.hold_writer().await;
        let restore = restore(&client, "S1", &iframes, &mut guard);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), restore)
                .await
                .is_err()
        );
        drop(guard);
        assert!(released.lock().unwrap().is_empty());
        drop(writer);
        for _ in 0..50 {
            if released.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut seen = released.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec![format!("S1:{GROUP}"), format!("S2:{GROUP}")]);
    }

    #[test]
    fn a_failed_frame_fails_the_restore() {
        let ok = vec![
            ("A".to_string(), Outcome::Restored),
            ("B".to_string(), Outcome::Gone),
            ("C".to_string(), Outcome::Clean),
        ];
        assert_eq!(report_result(&ok), Ok(()));
        let mut bad = ok.clone();
        bad.push(("D".to_string(), Outcome::Failed("kept".to_string())));
        assert_eq!(report_result(&bad), Err("frame D: kept".to_string()));
    }
}
