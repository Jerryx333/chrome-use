//! Teach by demonstration: turn a recording of what the user did in a page
//! into something that runs again without a model.
//!
//! The recording comes from `chrome-use teach start/stop` (the daemon side is
//! `native/teach.rs`) or from the side panel's Record button (the extension
//! records, the native host compiles through [`panel_handle`]). Both hand this
//! module the same shape: `{startUrl, events: [...]}`, where the events are
//! what `extensions/ab-connect/teach-recorder.js` reports plus `{t: "nav"}`
//! entries for main-frame navigations.
//!
//! The output is a `chrome-use script` JSON op-list (the default) or a site
//! adapter skeleton (`--as adapter`). Each element step picks one locator: a
//! role + accessible name when that name was unique on the page, then a form
//! label, a test id, a stable CSS selector (id, name), a placeholder, and last
//! a structural CSS path. Keystrokes collapse into one `fill`; a click that
//! only focused the field it then typed into is dropped; values typed into
//! password, card or one-time-code fields were never recorded and become
//! script parameters (`--arg` / `--arg-env`).

use serde_json::{json, Map, Value};

/// Where `teach stop` and the panel save flows when no path is given.
pub fn default_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".chrome-use").join("teach"))
}

/// A value the replay needs from whoever runs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    /// `secret` (typed into a password-like field) or `file` (an upload).
    pub kind: &'static str,
    pub hint: String,
}

#[derive(Debug, Clone)]
enum Act {
    Navigate(String),
    Click(Value),
    Fill(Value, FillValue),
    Select(Value, Vec<String>, Vec<String>),
    Check(Value, bool),
    Press(String),
    Upload(Value, String, Vec<String>),
    Scroll(i64),
    WaitUrl(String),
}

#[derive(Debug, Clone)]
enum FillValue {
    Text(String),
    Param(String),
}

/// The result of compiling one recording.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub name: String,
    pub start_url: String,
    pub final_url: String,
    /// The `chrome-use script` op-list.
    pub program: Value,
    /// One line per step, for review.
    pub steps: Vec<String>,
    pub params: Vec<Param>,
    acts: Vec<Act>,
}

fn loc_key(loc: &Value) -> String {
    loc.get("css")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}|{}|{}", s(loc, "role"), s(loc, "name"), s(loc, "label")))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// `scheme://host/path` without query or fragment: what a replay waits for
/// after a navigation, since query strings often carry per-visit tokens.
pub fn url_stem(url: &str) -> String {
    let no_frag = url.split('#').next().unwrap_or(url);
    no_frag.split('?').next().unwrap_or(no_frag).to_string()
}

fn same_page(a: &str, b: &str) -> bool {
    a.split('#').next() == b.split('#').next()
}

fn param_name(hint: &str, kind: &str, taken: &[Param]) -> String {
    let mut base: String = hint
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if base.is_empty() || base.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        base = if kind == "file" { "file" } else { "password" }.to_string();
    }
    if base.len() > 32 {
        base.truncate(32);
    }
    let mut name = base.clone();
    let mut n = 2;
    while taken.iter().any(|p| p.name == name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    name
}

/// Stand-in for a secret field's value in a recorded URL.
pub const REDACTED: &str = "__redacted__";

/// Blank out, in every recorded URL, the query values of fields the user
/// typed a secret into (a GET form puts a password in the next page's URL).
pub fn redact_recording(recording: &mut Value) {
    let names: Vec<String> = recording["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["secret"] == true)
        .filter_map(|e| e["field"].as_str().map(str::to_string))
        .collect();
    if names.is_empty() {
        return;
    }
    let redact = |url: &str| -> Option<String> {
        let mut parsed = url::Url::parse(url).ok()?;
        if !parsed
            .query_pairs()
            .any(|(k, _)| names.iter().any(|n| *n == k))
        {
            return None;
        }
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| {
                let hidden = names.iter().any(|n| *n == k);
                (
                    k.into_owned(),
                    if hidden {
                        REDACTED.to_string()
                    } else {
                        v.into_owned()
                    },
                )
            })
            .collect();
        parsed.query_pairs_mut().clear().extend_pairs(pairs);
        Some(parsed.to_string())
    };
    if let Some(events) = recording["events"].as_array_mut() {
        for e in events {
            if let Some(u) = e["url"].as_str().and_then(&redact) {
                e["url"] = json!(u);
            }
        }
    }
}

/// Compile a recording (`{startUrl, events}`) into a script.
pub fn compile(recording: &Value, name: &str) -> Result<Compiled, String> {
    let start_url = recording
        .get("startUrl")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .ok_or("recording has no startUrl")?
        .to_string();
    let mut recording = recording.clone();
    redact_recording(&mut recording);
    let mut events: Vec<Value> = recording
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Recorder events can be drained after a navigation event that followed
    // them; order by the time each happened (stable for equal times).
    events.sort_by_key(|e| e.get("ts").and_then(Value::as_i64).unwrap_or(0));

    let mut acts: Vec<Act> = vec![Act::Navigate(start_url.clone())];
    let mut params: Vec<Param> = Vec::new();
    let mut current = start_url.clone();
    let mut last_action_ts: Option<i64> = None;
    // A secret field keeps one parameter for the whole recording.
    let mut secret_params: Vec<(String, String)> = Vec::new();

    for ev in &events {
        let ts = ev.get("ts").and_then(Value::as_i64).unwrap_or(0);
        let loc = ev.get("loc").cloned().unwrap_or(Value::Null);
        let key = loc_key(&loc);
        match s(ev, "t") {
            "input" => {
                let value = if ev.get("secret").and_then(Value::as_bool) == Some(true) {
                    let existing = secret_params
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, p)| p.clone());
                    let p = existing.unwrap_or_else(|| {
                        let hint = s(ev, "hint");
                        let n = param_name(
                            if hint.is_empty() { "password" } else { hint },
                            "secret",
                            &params,
                        );
                        params.push(Param {
                            name: n.clone(),
                            kind: "secret",
                            hint: describe_loc(&loc),
                        });
                        secret_params.push((key.clone(), n.clone()));
                        n
                    });
                    FillValue::Param(p)
                } else {
                    FillValue::Text(s(ev, "value").to_string())
                };
                match acts.last_mut() {
                    Some(Act::Fill(l, v)) if loc_key(l) == key => *v = value,
                    _ => {
                        if matches!(acts.last(), Some(Act::Click(l)) if loc_key(l) == key) {
                            acts.pop();
                        }
                        acts.push(Act::Fill(loc, value));
                    }
                }
                last_action_ts = Some(ts);
            }
            "click" => {
                // Enter in a form makes the browser click its submit button.
                if matches!(acts.last(), Some(Act::Press(k)) if k == "Enter")
                    && last_action_ts.is_some_and(|t| ts.saturating_sub(t) < 300)
                {
                    continue;
                }
                // Clicking back into the field just typed into changes nothing.
                if matches!(acts.last(), Some(Act::Fill(l, _)) if loc_key(l) == key) {
                    continue;
                }
                acts.push(Act::Click(loc));
                last_action_ts = Some(ts);
            }
            "select" => {
                if matches!(acts.last(), Some(Act::Click(l)) if loc_key(l) == key) {
                    acts.pop();
                }
                let strings = |k: &str| -> Vec<String> {
                    ev.get(k)
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                match acts.last_mut() {
                    Some(Act::Select(l, v, labels)) if loc_key(l) == key => {
                        *v = strings("values");
                        *labels = strings("labels");
                    }
                    _ => acts.push(Act::Select(loc, strings("values"), strings("labels"))),
                }
                last_action_ts = Some(ts);
            }
            "check" => {
                let checked = ev.get("checked").and_then(Value::as_bool).unwrap_or(true);
                match acts.last_mut() {
                    Some(Act::Check(l, c)) if loc_key(l) == key => *c = checked,
                    _ => acts.push(Act::Check(loc, checked)),
                }
                last_action_ts = Some(ts);
            }
            "key" => {
                acts.push(Act::Press(s(ev, "key").to_string()));
                last_action_ts = Some(ts);
            }
            "upload" => {
                let files: Vec<String> = ev
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if files.is_empty() {
                    continue;
                }
                let n = param_name("file", "file", &params);
                params.push(Param {
                    name: n.clone(),
                    kind: "file",
                    hint: format!("recorded with {}", files.join(", ")),
                });
                acts.push(Act::Upload(loc, n, files));
                last_action_ts = Some(ts);
            }
            "scroll" => {
                let dy = ev.get("dy").and_then(Value::as_i64).unwrap_or(0);
                if dy > 0 {
                    match acts.last_mut() {
                        Some(Act::Scroll(total)) => *total += dy,
                        _ => acts.push(Act::Scroll(dy)),
                    }
                }
            }
            "nav" => {
                let url = s(ev, "url").to_string();
                if url.is_empty() || url.starts_with("about:") || url.starts_with("chrome") {
                    continue;
                }
                let same_doc = ev.get("sameDocument").and_then(Value::as_bool) == Some(true);
                let reason = s(ev, "reason");
                // A fragment, or a page rewriting its own query (history API),
                // is not a step.
                if reason != "reload"
                    && (same_page(&url, &current)
                        || (same_doc && url_stem(&url) == url_stem(&current)))
                {
                    current = url;
                    continue;
                }
                let triggered = last_action_ts
                    .is_some_and(|t| ts.saturating_sub(t) <= if same_doc { 4000 } else { 8000 })
                    && !matches!(acts.last(), Some(Act::Navigate(_) | Act::WaitUrl(_)))
                    && reason != "typed";
                if triggered {
                    // Same path, new query (a search on the same page): wait
                    // for the whole URL, or the wait would pass at once.
                    let pattern = if url_stem(&url) == url_stem(&current) && !url.contains(REDACTED)
                    {
                        url.split('#').next().unwrap_or(&url).to_string()
                    } else {
                        url_stem(&url)
                    };
                    acts.push(Act::WaitUrl(pattern));
                } else if !same_doc {
                    // Typed into the address bar, a bookmark, a reload.
                    if matches!(acts.last(), Some(Act::Navigate(_))) {
                        acts.pop();
                    }
                    acts.push(Act::Navigate(url.clone()));
                }
                current = url;
                if !same_doc {
                    last_action_ts = None;
                }
            }
            _ => {}
        }
    }
    if acts.len() < 2 {
        return Err(
            "nothing to replay: no clicks, typing, choices or navigations were recorded".into(),
        );
    }
    let final_url = current;
    let program = build_program(name, &start_url, &final_url, &acts, &params);
    let steps = acts.iter().map(describe).collect();
    Ok(Compiled {
        name: name.to_string(),
        start_url,
        final_url,
        program,
        steps,
        params,
        acts,
    })
}

const AX_ROLES: &[&str] = &[
    "button",
    "link",
    "checkbox",
    "radio",
    "switch",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "treeitem",
    "textbox",
    "searchbox",
    "combobox",
    "listbox",
    "spinbutton",
    "slider",
];

/// One element step in the op-list. `sub` is click, fill, check or uncheck.
/// The most robust locator leads; up to two others follow in `or`, tried
/// only when the one before found nothing to act on.
fn element_op(loc: &Value, sub: &str, value: Option<Value>) -> Value {
    let role = s(loc, "role");
    let name = s(loc, "name");
    let label = s(loc, "label");
    let css = s(loc, "css");
    let stable = loc.get("stable").and_then(Value::as_bool) == Some(true);
    let unique = loc.get("nameUnique").and_then(Value::as_bool) == Some(true);
    let secret_field = s(loc, "type") == "password";
    let fill = sub == "fill";
    let mut candidates: Vec<Value> = Vec::new();
    // A text field is found by its accessible name whatever its role: pages
    // switch a search box between searchbox and combobox as it gains focus.
    let field_name = if !label.is_empty() { label } else { name };
    if fill && !field_name.is_empty() && (!label.is_empty() || unique) {
        candidates.push(json!({"do": "getbylabel", "label": field_name, "subaction": sub}));
    }
    if !role.is_empty() && !name.is_empty() && unique && AX_ROLES.contains(&role) && !secret_field {
        candidates.push(json!({"do": "getbyrole", "role": role, "name": name, "subaction": sub}));
    }
    if !s(loc, "testid").is_empty() {
        candidates.push(json!({"do": "getbytestid", "testId": s(loc, "testid"), "subaction": sub}));
    }
    if !css.is_empty() && stable {
        candidates.push(json!({"do": sub, "selector": css}));
    }
    if !s(loc, "placeholder").is_empty() && fill {
        candidates.push(
            json!({"do": "getbyplaceholder", "placeholder": s(loc, "placeholder"), "subaction": sub}),
        );
    }
    if !css.is_empty() && !stable {
        candidates.push(json!({"do": sub, "selector": css}));
    }
    if candidates.is_empty() {
        candidates.push(if !role.is_empty() && !name.is_empty() {
            json!({"do": "getbyrole", "role": role, "name": name, "subaction": sub})
        } else {
            json!({"do": sub, "selector": format!("text={}", name)})
        });
    }
    // Keep a CSS selector among the alternatives whenever there is one.
    let mut chosen: Vec<Value> = candidates.iter().take(2).cloned().collect();
    if let Some(css_op) = candidates
        .iter()
        .skip(2)
        .find(|c| c.get("selector").is_some())
    {
        if !chosen.iter().any(|c| c.get("selector").is_some()) {
            chosen.push(css_op.clone());
        }
    }
    if let Some(v) = value {
        for c in &mut chosen {
            c["value"] = v.clone();
        }
    }
    let mut op = chosen.remove(0);
    if !chosen.is_empty() {
        op["or"] = Value::Array(chosen);
    }
    op
}

fn build_program(
    name: &str,
    start_url: &str,
    final_url: &str,
    acts: &[Act],
    params: &[Param],
) -> Value {
    let mut ops: Vec<Value> = Vec::new();
    ops.push(json!({"log": format!(
        "chrome-use teach: \"{}\" recorded {} on {}",
        name,
        chrono::Local::now().format("%Y-%m-%d"),
        url_stem(start_url)
    )}));
    for p in params {
        let how = if p.kind == "file" {
            format!(
                "pass --arg {}=<path of the file to upload> ({})",
                p.name, p.hint
            )
        } else {
            format!(
                "pass --arg-env {}=<ENV VAR holding it> (typed into {}; never stored)",
                p.name, p.hint
            )
        };
        ops.push(json!({"assert": {"ne": [format!("{{{{{}}}}}", p.name), null]}, "msg": format!("missing parameter `{}`: {}", p.name, how)}));
    }
    for act in acts {
        match act {
            Act::Navigate(url) => {
                ops.push(json!({"do": "navigate", "url": url}));
            }
            Act::WaitUrl(url) => {
                ops.push(json!({"do": "waitforurl", "url": url, "timeout": 20000}));
                ops.push(json!({"do": "waitforloadstate", "state": "load", "timeout": 20000}));
            }
            Act::Click(loc) => ops.push(element_op(loc, "click", None)),
            Act::Fill(loc, v) => {
                let value = match v {
                    FillValue::Text(t) => json!(t),
                    FillValue::Param(p) => json!(format!("{{{{{p}}}}}")),
                };
                ops.push(element_op(loc, "fill", Some(value)));
            }
            Act::Check(loc, checked) => {
                ops.push(element_op(
                    loc,
                    if *checked { "check" } else { "uncheck" },
                    None,
                ));
            }
            Act::Select(loc, values, _) => {
                let css = s(loc, "css");
                ops.push(json!({"do": "select", "selector": css, "values": values}));
            }
            Act::Press(key) => ops.push(json!({"do": "press", "key": key})),
            Act::Upload(loc, param, _) => {
                ops.push(json!({"do": "upload", "selector": s(loc, "css"), "files": [format!("{{{{{param}}}}}")]}));
            }
            Act::Scroll(dy) => {
                ops.push(json!({"do": "scroll", "direction": "down", "amount": dy}));
                ops.push(json!({"do": "wait", "timeout": 800}));
            }
        }
    }
    ops.push(json!({"do": "url", "bind": "final"}));
    let stem = url_stem(final_url);
    ops.push(json!({"assert": {"contains": ["{{final.url}}", stem]}, "msg": format!("replay did not end on {stem}")}));
    ops.push(json!({"return": {"ok": true, "url": "{{final.url}}"}}));
    Value::Array(ops)
}

fn describe_loc(loc: &Value) -> String {
    let role = s(loc, "role");
    let name = s(loc, "name");
    let label = s(loc, "label");
    let what = if !role.is_empty() {
        role
    } else {
        s(loc, "tag")
    };
    if !name.is_empty() {
        format!("{what} \u{201c}{name}\u{201d}")
    } else if !label.is_empty() {
        format!("{what} \u{201c}{label}\u{201d}")
    } else if !s(loc, "css").is_empty() {
        format!("{what} {}", s(loc, "css"))
    } else {
        what.to_string()
    }
}

fn describe(act: &Act) -> String {
    match act {
        Act::Navigate(url) => format!("Open {url}"),
        Act::WaitUrl(url) => format!("Wait for {url}"),
        Act::Click(loc) => format!("Click {}", describe_loc(loc)),
        Act::Fill(loc, FillValue::Text(t)) => {
            format!("Type \u{201c}{t}\u{201d} into {}", describe_loc(loc))
        }
        Act::Fill(loc, FillValue::Param(p)) => {
            format!(
                "Type the `{p}` parameter into {} (secret; not stored)",
                describe_loc(loc)
            )
        }
        Act::Select(loc, _, labels) => {
            format!(
                "Choose \u{201c}{}\u{201d} in {}",
                labels.join(", "),
                describe_loc(loc)
            )
        }
        Act::Check(loc, true) => format!("Check {}", describe_loc(loc)),
        Act::Check(loc, false) => format!("Uncheck {}", describe_loc(loc)),
        Act::Press(key) => format!("Press {key}"),
        Act::Upload(loc, p, files) => format!(
            "Upload the `{p}` parameter into {} (recorded: {})",
            describe_loc(loc),
            files.join(", ")
        ),
        Act::Scroll(dy) => format!("Scroll down {dy}px (the page loaded more)"),
    }
}

// --- site adapter skeleton ------------------------------------------------

/// The `site/command` name and domain an adapter skeleton gets.
pub fn adapter_names(compiled: &Compiled, flow: &str) -> (String, String) {
    let host = url::Url::parse(&compiled.start_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "localhost".to_string());
    let site: String = host
        .trim_start_matches("www.")
        .split('.')
        .next()
        .unwrap_or("site")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let cmd: String = flow
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let cmd = if cmd.trim_matches('-').is_empty() {
        "flow".to_string()
    } else {
        cmd.trim_matches('-').to_string()
    };
    (format!("{site}/{cmd}"), host)
}

/// A site adapter that replays the recording through `args.ui` (trusted CDP
/// input), one page per run: a step that navigates ends the run with
/// `status: "incomplete"`, so run it with `--until-done`.
pub fn to_adapter(compiled: &Compiled, flow: &str) -> String {
    let (full_name, domain) = adapter_names(compiled, flow);
    let mut args = Map::new();
    for p in &compiled.params {
        let mut spec = json!({"required": true, "description": if p.kind == "file" {
            format!("file to upload ({})", p.hint)
        } else {
            format!("secret typed into {}; pass it with --{} @- to read stdin", p.hint, p.name)
        }});
        if p.kind == "file" {
            spec["type"] = json!("file");
        }
        args.insert(p.name.clone(), spec);
    }
    // Pages: each starts where a navigation landed.
    let mut pages: Vec<Value> = Vec::new();
    let mut page = json!({"url": url_stem(&compiled.start_url), "steps": []});
    let mut typed_args = 0;
    for act in &compiled.acts {
        let step = match act {
            Act::Navigate(url) => {
                if page["steps"].as_array().is_some_and(|a| !a.is_empty()) {
                    pages.push(page);
                }
                page = json!({"url": url_stem(url), "steps": []});
                continue;
            }
            Act::WaitUrl(url) => {
                page["navigates"] = json!(true);
                pages.push(page);
                page = json!({"url": url_stem(url), "steps": []});
                continue;
            }
            Act::Click(loc) => json!({"op": "click", "loc": slim(loc)}),
            Act::Fill(loc, FillValue::Text(t)) => {
                typed_args += 1;
                let name = format!("text{typed_args}");
                args.insert(
                    name.clone(),
                    json!({"required": false, "description": format!("typed into {} (recorded: {:?})", describe_loc(loc), t)}),
                );
                json!({"op": "fill", "loc": slim(loc), "arg": name, "value": t})
            }
            Act::Fill(loc, FillValue::Param(p)) => {
                json!({"op": "fill", "loc": slim(loc), "arg": p})
            }
            Act::Check(loc, c) => json!({"op": "check", "loc": slim(loc), "checked": c}),
            Act::Select(loc, v, l) => {
                json!({"op": "select", "loc": slim(loc), "values": v, "labels": l})
            }
            Act::Press(k) => json!({"op": "press", "key": k}),
            Act::Upload(loc, p, _) => json!({"op": "upload", "loc": slim(loc), "arg": p}),
            Act::Scroll(dy) => json!({"op": "scroll", "dy": dy}),
        };
        page["steps"].as_array_mut().unwrap().push(step);
    }
    // The page the flow ends on, even with nothing left to do there: a run
    // that lands on it is done.
    pages.push(page);
    let meta = json!({
        "name": full_name,
        "description": format!("Recorded with chrome-use teach ({}); review before relying on it", compiled.name),
        "domain": domain,
        "readOnly": false,
        "world": "isolated",
        "timeout": 120,
        "args": Value::Object(args),
    });
    let steps_comment: String = compiled
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("//   {}. {}\n", i + 1, s.replace("*/", "* /")))
        .collect();
    format!(
        "/* @meta\n{meta}\n*/\n\
// Recorded by `chrome-use teach` from {start}. What it does:\n{steps_comment}\
// A step that navigates ends the run with status \"incomplete\": run it with\n\
// `chrome-use site {full_name} --until-done`. Input goes through args.ui\n\
// (trusted CDP input); nothing here dispatches synthetic events.\n\
async function(args) {{\n\
  const PAGES = {pages};\n\
  const norm = s => (s == null ? '' : String(s)).replace(/\\s+/g, ' ').trim();\n\
  const sleep = ms => new Promise(r => setTimeout(r, ms));\n\
  const roleOf = el => el.getAttribute('role') || ({{A: el.hasAttribute('href') ? 'link' : '', BUTTON: 'button', SELECT: 'combobox', TEXTAREA: 'textbox'}})[el.tagName] ||\n\
    (el.tagName === 'INPUT' ? ({{button: 'button', submit: 'button', reset: 'button', checkbox: 'checkbox', radio: 'radio', search: 'searchbox'}})[el.type] || 'textbox' : '');\n\
  const nameOf = el => norm(el.getAttribute('aria-label')) || norm(Array.from(el.labels || []).map(l => l.innerText).join(' ')) ||\n\
    norm(el.innerText || el.value) || norm(el.getAttribute('title')) || norm(el.getAttribute('placeholder'));\n\
  function find(loc) {{\n\
    if (loc.css) {{ const all = document.querySelectorAll(loc.css); if (all.length === 1) return all[0]; }}\n\
    if (loc.role && loc.name) {{\n\
      const hit = Array.from(document.querySelectorAll('a,button,input,select,textarea,summary,[role]')).filter(el => roleOf(el) === loc.role && nameOf(el).includes(loc.name));\n\
      if (hit.length === 1) return hit[0];\n\
    }}\n\
    if (loc.label) {{\n\
      const hit = Array.from(document.querySelectorAll('input,select,textarea,[contenteditable]')).filter(el => nameOf(el).includes(loc.label));\n\
      if (hit.length === 1) return hit[0];\n\
    }}\n\
    return null;\n\
  }}\n\
  async function waitFor(loc) {{\n\
    for (let i = 0; i < 50; i++) {{ const el = find(loc); if (el) return el; await sleep(200); }}\n\
    throw new Error('not found: ' + JSON.stringify(loc));\n\
  }}\n\
  const here = location.origin + location.pathname;\n\
  let index = PAGES.findIndex(p => p.url === here);\n\
  if (index < 0) {{ location.href = PAGES[0].url; return {{ status: 'incomplete', note: 'opened the start page' }}; }}\n\
  for (const step of PAGES[index].steps) {{\n\
    if (step.op === 'press') {{ await args.ui.press(step.key); continue; }}\n\
    if (step.op === 'scroll') {{ await args.ui.scroll(step.dy); await sleep(800); continue; }}\n\
    const el = await waitFor(step.loc);\n\
    if (step.op === 'click') await args.ui.click(el);\n\
    else if (step.op === 'fill') await args.ui.type(el, String(args[step.arg] ?? step.value ?? ''), {{ clear: true }});\n\
    else if (step.op === 'check') {{ if (el.checked !== step.checked) await args.ui.click(el); }}\n\
    else if (step.op === 'select') {{ await args.ui.click(el); await args.ui.typeHere(step.labels[0] || step.values[0]); await args.ui.press('Enter'); }}\n\
    else if (step.op === 'upload') await args[step.arg].setOn(el);\n\
  }}\n\
  if (PAGES[index].navigates) return {{ status: 'incomplete', page: index + 1, of: PAGES.length }};\n\
  return {{ status: 'done', url: location.href }};\n\
}}\n",
        meta = serde_json::to_string_pretty(&meta).unwrap_or_default(),
        start = url_stem(&compiled.start_url),
        pages = serde_json::to_string(&pages).unwrap_or_default(),
    )
}

/// The locator fields an adapter needs.
fn slim(loc: &Value) -> Value {
    let mut out = Map::new();
    for k in ["css", "role", "name", "label"] {
        if let Some(v) = loc
            .get(k)
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        {
            out.insert(k.to_string(), v.clone());
        }
    }
    Value::Object(out)
}

// --- output ---------------------------------------------------------------

/// How a recording is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    Script,
    Adapter,
}

impl Format {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "script" | "json" => Ok(Self::Script),
            "adapter" | "site" => Ok(Self::Adapter),
            other => Err(format!(
                "unknown teach format `{other}`; use --as script or --as adapter"
            )),
        }
    }
}

/// A file-system-safe flow name.
pub fn safe_name(name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let n = n.trim_matches('-').to_string();
    if n.is_empty() {
        format!("flow-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
    } else {
        n
    }
}

/// The text a format produces and the default path it is saved to.
pub fn render(compiled: &Compiled, format: Format) -> Result<(String, std::path::PathBuf), String> {
    let dir = default_dir().ok_or("cannot resolve the home directory")?;
    let flow = safe_name(&compiled.name);
    Ok(match format {
        Format::Script => (
            serde_json::to_string_pretty(&compiled.program).unwrap_or_default() + "\n",
            dir.join(format!("{flow}.json")),
        ),
        Format::Adapter => {
            let (full, _) = adapter_names(compiled, &flow);
            let path = dirs::home_dir()
                .ok_or("cannot resolve the home directory")?
                .join(".chrome-use")
                .join("my-sites")
                .join(format!("{full}.js"));
            (to_adapter(compiled, &flow), path)
        }
    })
}

/// Write the output, refusing to overwrite unless asked.
pub fn write_output(path: &std::path::Path, text: &str, force: bool) -> Result<(), String> {
    if path.exists() && !force {
        return Err(format!(
            "{} already exists; pass --force to replace it or --out to choose another path",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// How to run what was written, for the CLI and the panel.
pub fn run_hint(compiled: &Compiled, format: Format, path: &std::path::Path) -> String {
    match format {
        Format::Script => {
            let mut cmd = format!("chrome-use script {}", path.display());
            for p in &compiled.params {
                if p.kind == "file" {
                    cmd.push_str(&format!(" --arg {}=<file>", p.name));
                } else {
                    cmd.push_str(&format!(" --arg-env {}=<ENV_VAR>", p.name));
                }
            }
            cmd
        }
        Format::Adapter => {
            let (full, _) = adapter_names(compiled, &safe_name(&compiled.name));
            let dir = path
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            format!(
                "chrome-use site add {dir} && chrome-use site update && chrome-use site {full} --until-done"
            )
        }
    }
}

/// The JSON summary `teach stop --json` and the panel show.
pub fn summary(compiled: &Compiled) -> Value {
    json!({
        "name": compiled.name,
        "startUrl": compiled.start_url,
        "finalUrl": compiled.final_url,
        "steps": compiled.steps,
        "params": compiled.params.iter().map(|p| json!({"name": p.name, "kind": p.kind, "hint": p.hint})).collect::<Vec<_>>(),
        "program": compiled.program,
    })
}

// --- CLI ----------------------------------------------------------------

/// `teach start|stop|status|cancel` from the command line. Returns the exit
/// code. `teach stop` compiles the recording here, in the CLI process, and
/// writes the file; the daemon only records.
pub fn run_cli(json_out: bool, session: &str, cmd: Value) -> i32 {
    let action = cmd["action"].as_str().unwrap_or("").to_string();
    let fail = |msg: &str| -> i32 {
        if json_out {
            println!("{}", json!({"success": false, "data": null, "error": msg}));
        } else {
            eprintln!("{} {}", crate::color::error_indicator(), msg);
        }
        1
    };
    let resp = match crate::connection::send_command(cmd.clone(), session) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    if !resp.success {
        return fail(resp.error.as_deref().unwrap_or("teach failed"));
    }
    let data = resp.data.clone().unwrap_or(Value::Null);
    match action.as_str() {
        "teach_start" => {
            if json_out {
                println!("{}", json!({"success": true, "data": data, "error": null}));
            } else {
                println!(
                    "{} recording \u{201c}{}\u{201d} on {}",
                    crate::color::success_indicator(),
                    data["name"].as_str().unwrap_or(""),
                    data["startUrl"].as_str().unwrap_or("")
                );
                println!("  Do the flow by hand in that tab, then run `chrome-use teach stop`.");
                if let Some(w) = data["warning"].as_str() {
                    eprintln!("{} {}", crate::color::warning_indicator(), w);
                }
            }
            0
        }
        "teach_status" => {
            if json_out {
                println!("{}", json!({"success": true, "data": data, "error": null}));
            } else if data["recording"] == true {
                println!(
                    "recording \u{201c}{}\u{201d}: {} actions so far ({}s)",
                    data["name"].as_str().unwrap_or(""),
                    data["events"],
                    data["elapsedMs"].as_u64().unwrap_or(0) / 1000
                );
            } else {
                println!("not recording");
            }
            0
        }
        "teach_stop" if data["discarded"] == true => {
            if json_out {
                println!("{}", json!({"success": true, "data": data, "error": null}));
            } else {
                println!("{} recording discarded", crate::color::success_indicator());
            }
            0
        }
        "teach_stop" => {
            let name = cmd["name"]
                .as_str()
                .or(data["name"].as_str())
                .unwrap_or("flow")
                .to_string();
            let compiled = match compile(&data["recording"], &name) {
                Ok(c) => c,
                Err(e) => return fail(&format!("teach stop: {e}")),
            };
            let format = match Format::parse(cmd["as"].as_str().unwrap_or("script")) {
                Ok(f) => f,
                Err(e) => return fail(&e),
            };
            let (text, default_path) = match render(&compiled, format) {
                Ok(r) => r,
                Err(e) => return fail(&e),
            };
            if cmd["print"] == true {
                print!("{text}");
                return 0;
            }
            let path = cmd["out"]
                .as_str()
                .map(std::path::PathBuf::from)
                .unwrap_or(default_path);
            // Keep the raw recording next to the output, so it can be
            // compiled again (`teach compile`) after a fix to the compiler.
            if let Err(e) = write_output(&path, &text, cmd["force"] == true) {
                return fail(&e);
            }
            let raw = path.with_extension("recording.json");
            let _ = std::fs::write(
                &raw,
                serde_json::to_string_pretty(&data["recording"]).unwrap_or_default(),
            );
            let run = run_hint(&compiled, format, &path);
            if json_out {
                let mut out = summary(&compiled);
                out["path"] = json!(path.display().to_string());
                out["recordingPath"] = json!(raw.display().to_string());
                out["format"] = json!(if format == Format::Adapter {
                    "adapter"
                } else {
                    "script"
                });
                out["run"] = json!(run);
                out["warning"] = data["warning"].clone();
                println!("{}", json!({"success": true, "data": out, "error": null}));
            } else {
                println!(
                    "{} {} steps \u{2192} {}",
                    crate::color::success_indicator(),
                    compiled.steps.len(),
                    path.display()
                );
                for (i, step) in compiled.steps.iter().enumerate() {
                    println!("  {:>2}. {}", i + 1, step);
                }
                for p in &compiled.params {
                    println!("  parameter `{}` ({}): {}", p.name, p.kind, p.hint);
                }
                println!("Review it, then run: {run}");
                if let Some(w) = data["warning"].as_str() {
                    eprintln!("{} {}", crate::color::warning_indicator(), w);
                }
            }
            0
        }
        _ => fail("unknown teach action"),
    }
}

/// `teach compile <recording.json> [--name N] [--format script|adapter] [--out F]`:
/// compile a saved raw recording again, without a browser.
pub fn run_compile(json_out: bool, args: &[String]) -> i32 {
    let mut file = None;
    let mut name = None;
    let mut format = "script".to_string();
    let mut out = None;
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                name = args.get(i + 1).cloned();
                i += 1;
            }
            "--format" => {
                format = args.get(i + 1).cloned().unwrap_or_default();
                i += 1;
            }
            "--adapter" => format = "adapter".into(),
            "--out" => {
                out = args.get(i + 1).cloned();
                i += 1;
            }
            "--force" => force = true,
            "--json" => {}
            a if !a.starts_with("--") && file.is_none() => file = Some(a.to_string()),
            a => {
                eprintln!("teach compile: unexpected argument `{a}`");
                return 2;
            }
        }
        i += 1;
    }
    let result = (|| -> Result<Value, String> {
        let file = file.ok_or(
            "usage: teach compile <recording.json> [--name N] [--format script|adapter] [--out F]",
        )?;
        let raw = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {file}: {e}"))?;
        let recording: Value =
            serde_json::from_str(&raw).map_err(|e| format!("{file} is not JSON: {e}"))?;
        let stem = std::path::Path::new(&file)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("flow")
            .trim_end_matches(".json")
            .trim_end_matches(".recording")
            .to_string();
        let compiled = compile(&recording, name.as_deref().unwrap_or(&stem))?;
        let format = Format::parse(&format)?;
        let (text, default_path) = render(&compiled, format)?;
        let path = out.map(std::path::PathBuf::from).unwrap_or(default_path);
        write_output(&path, &text, force)?;
        let mut s = summary(&compiled);
        s["path"] = json!(path.display().to_string());
        s["run"] = json!(run_hint(&compiled, format, &path));
        Ok(s)
    })();
    match result {
        Ok(s) => {
            if json_out {
                println!("{}", json!({"success": true, "data": s, "error": null}));
            } else {
                println!(
                    "{} wrote {}",
                    crate::color::success_indicator(),
                    s["path"].as_str().unwrap_or("")
                );
                println!("Run: {}", s["run"].as_str().unwrap_or(""));
            }
            0
        }
        Err(e) => {
            if json_out {
                println!("{}", json!({"success": false, "data": null, "error": e}));
            } else {
                eprintln!("{} {}", crate::color::error_indicator(), e);
            }
            1
        }
    }
}

// --- side panel host operations --------------------------------------------

/// `teachCompile` and `teachSave` from the side panel's Record button
/// (through the native host).
pub fn panel_handle(operation: &str, params: &Value) -> Result<Value, String> {
    match operation {
        "teachCompile" => {
            let name = params["name"].as_str().unwrap_or("recording");
            let compiled = compile(&params["recording"], name)?;
            let mut out = summary(&compiled);
            out["schemaVersion"] = json!(1);
            Ok(out)
        }
        "teachSave" => {
            let name = safe_name(params["name"].as_str().unwrap_or(""));
            let program = params["program"].clone();
            if !program.is_array() {
                return Err("teachSave expects the reviewed op-list as `program`".into());
            }
            // The reviewed program must still be a valid script.
            let text = serde_json::to_string_pretty(&program).unwrap_or_default() + "\n";
            let path = default_dir()
                .ok_or("cannot resolve the home directory")?
                .join(format!("{name}.json"));
            let force = params["force"].as_bool() == Some(true);
            write_output(&path, &text, force)?;
            Ok(
                json!({"schemaVersion": 1, "path": path.display().to_string(), "run": format!("chrome-use script {}", path.display())}),
            )
        }
        other => Err(format!("unknown teach operation {other}")),
    }
}

/// The recorder evaluated in an isolated world (shared with the extension).
pub const RECORDER_JS: &str = include_str!("../../extensions/ab-connect/teach-recorder.js");

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(role: &str, name: &str, css: &str) -> Value {
        json!({"role": role, "name": name, "css": css, "stable": css.starts_with('#'), "nameUnique": true, "tag": "x"})
    }

    fn rec(events: Vec<Value>) -> Value {
        json!({"startUrl": "http://site.test/form?x=1", "events": events})
    }

    #[test]
    fn keystrokes_collapse_into_one_fill_and_focus_click_is_dropped() {
        let name = json!({"role": "textbox", "name": "Name", "label": "Name", "css": "#name", "stable": true, "nameUnique": true});
        let c = compile(
            &rec(vec![
                json!({"t": "click", "loc": name, "field": true, "ts": 1}),
                json!({"t": "input", "loc": name, "value": "A", "ts": 2}),
                json!({"t": "input", "loc": name, "value": "Al", "ts": 3}),
                json!({"t": "input", "loc": name, "value": "Alice", "ts": 4}),
                json!({"t": "click", "loc": name, "field": true, "ts": 5}),
                json!({"t": "click", "loc": loc("button", "Submit", "form > button"), "ts": 6}),
            ]),
            "f",
        )
        .unwrap();
        let ops = c.program.as_array().unwrap();
        let fills: Vec<&Value> = ops
            .iter()
            .filter(|o| o["subaction"] == "fill" || o["do"] == "fill")
            .collect();
        assert_eq!(fills.len(), 1, "{ops:#?}");
        assert_eq!(fills[0]["value"], "Alice");
        assert_eq!(fills[0]["do"], "getbylabel");
        assert_eq!(fills[0]["label"], "Name");
        let clicks: Vec<&Value> = ops.iter().filter(|o| o["subaction"] == "click").collect();
        assert_eq!(clicks.len(), 1);
        assert_eq!(clicks[0]["name"], "Submit");
        assert_eq!(ops[1]["do"], "navigate");
        assert_eq!(c.steps.len(), 3);
    }

    #[test]
    fn secrets_become_parameters_and_are_never_in_the_program() {
        let pw = json!({"tag": "input", "type": "password", "label": "Password", "css": "#pw", "stable": true});
        let c = compile(
            &rec(vec![
                json!({"t": "input", "loc": pw, "secret": true, "hint": "pw", "ts": 1}),
                json!({"t": "input", "loc": pw, "secret": true, "hint": "pw", "ts": 2}),
                json!({"t": "key", "key": "Enter", "ts": 3}),
            ]),
            "login",
        )
        .unwrap();
        assert_eq!(c.params.len(), 1);
        assert_eq!(c.params[0].name, "pw");
        let text = c.program.to_string();
        assert!(text.contains("{{pw}}"), "{text}");
        let assert = &c.program[1];
        assert_eq!(assert["assert"]["ne"][0], "{{pw}}");
        // Password fields use the label, never a role name.
        let fill = c
            .program
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["value"] == "{{pw}}")
            .unwrap();
        assert_eq!(fill["do"], "getbylabel");
    }

    #[test]
    fn a_click_that_navigates_waits_for_the_new_page_and_typed_navigation_opens_it() {
        let c = compile(
            &rec(vec![
                json!({"t": "click", "loc": loc("link", "Next", "#next"), "ts": 1000}),
                json!({"t": "nav", "url": "http://site.test/step2?token=9", "ts": 1300}),
                json!({"t": "nav", "url": "http://other.test/", "reason": "typed", "ts": 30000}),
            ]),
            "f",
        )
        .unwrap();
        let ops = c.program.as_array().unwrap();
        let wait = ops.iter().find(|o| o["do"] == "waitforurl").unwrap();
        assert_eq!(wait["url"], "http://site.test/step2");
        let navs: Vec<&Value> = ops.iter().filter(|o| o["do"] == "navigate").collect();
        assert_eq!(navs.len(), 2);
        assert_eq!(navs[1]["url"], "http://other.test/");
        let last = &ops[ops.len() - 2];
        assert_eq!(last["assert"]["contains"][1], "http://other.test/");
    }

    #[test]
    fn non_unique_names_fall_back_to_css_and_select_uses_css() {
        let mut l = loc("button", "Add", "div:nth-of-type(2) > button");
        l["nameUnique"] = json!(false);
        let sel = json!({"tag": "select", "role": "combobox", "css": "#color", "stable": true});
        let c = compile(
            &rec(vec![
                json!({"t": "click", "loc": l, "ts": 1}),
                json!({"t": "select", "loc": sel, "values": ["r"], "labels": ["Red"], "ts": 2}),
                json!({"t": "check", "loc": loc("checkbox", "Agree", "#agree"), "checked": true, "ts": 3}),
            ]),
            "f",
        )
        .unwrap();
        let ops = c.program.as_array().unwrap();
        assert!(ops
            .iter()
            .any(|o| o["do"] == "click" && o["selector"] == "div:nth-of-type(2) > button"));
        assert!(ops
            .iter()
            .any(|o| o["do"] == "select" && o["selector"] == "#color" && o["values"][0] == "r"));
        assert!(ops
            .iter()
            .any(|o| o["do"] == "getbyrole" && o["subaction"] == "check"));
    }

    #[test]
    fn implicit_submit_and_history_rewrites_add_no_steps() {
        let q = json!({"role": "combobox", "name": "Search", "css": "#q", "stable": true, "nameUnique": true});
        let c = compile(
            &json!({"startUrl": "https://w.test/wiki/Special:Search", "events": [
                {"t": "input", "loc": q, "value": "rust", "ts": 100},
                {"t": "key", "key": "Enter", "ts": 200},
                {"t": "click", "loc": loc("button", "Search", "form > button"), "ts": 260},
                {"t": "nav", "url": "https://w.test/w/index.php?search=rust", "reason": "formSubmissionGet", "ts": 900},
                {"t": "nav", "url": "https://w.test/w/index.php?search=rust&ns0=1", "sameDocument": true, "ts": 950},
                {"t": "click", "loc": loc("link", "Rust", "a.hit"), "ts": 2000},
                {"t": "nav", "url": "https://w.test/w/index.php?search=rust&x=2", "sameDocument": true, "ts": 2005},
                {"t": "nav", "url": "https://w.test/wiki/Rust?wprov=1", "reason": "anchorClick", "ts": 2600},
                {"t": "nav", "url": "https://w.test/wiki/Rust", "sameDocument": true, "ts": 2700}
            ]}),
            "s",
        )
        .unwrap();
        assert_eq!(
            c.steps,
            vec![
                "Open https://w.test/wiki/Special:Search",
                "Type \u{201c}rust\u{201d} into combobox \u{201c}Search\u{201d}",
                "Press Enter",
                "Wait for https://w.test/w/index.php",
                "Click link \u{201c}Rust\u{201d}",
                "Wait for https://w.test/wiki/Rust",
            ]
        );
        // Semantic first, the CSS selector as the fallback.
        let fill = &c.program.as_array().unwrap()[2];
        assert_eq!(fill["do"], "getbylabel");
        assert_eq!(fill["label"], "Search");
        assert_eq!(fill["or"][0]["do"], "getbyrole");
        assert_eq!(fill["or"][1]["selector"], "#q");
        assert_eq!(fill["or"][1]["value"], "rust");
    }

    #[test]
    fn secrets_in_get_form_urls_are_redacted() {
        let mut r = json!({"startUrl": "http://a.test/", "events": [
            {"t": "input", "secret": true, "field": "pw", "ts": 1},
            {"t": "click", "loc": loc("button", "Go", "#go"), "ts": 2, "url": "http://a.test/"},
            {"t": "nav", "url": "http://a.test/?name=Ada&pw=hunter2", "reason": "formSubmissionGet", "ts": 3}
        ]});
        let c = compile(&r, "f").unwrap();
        assert!(!c.program.to_string().contains("hunter2"));
        redact_recording(&mut r);
        assert!(!r.to_string().contains("hunter2"), "{r}");
        assert!(r.to_string().contains("name=Ada"));
    }

    #[test]
    fn empty_recordings_are_refused() {
        assert!(compile(&rec(vec![]), "f").is_err());
        assert!(compile(&json!({"events": []}), "f").is_err());
    }

    #[test]
    fn adapter_skeleton_has_meta_pages_and_trusted_input_only() {
        let c = compile(
            &rec(vec![
                json!({"t": "input", "loc": loc("searchbox", "Search", "#q"), "value": "rust", "ts": 1}),
                json!({"t": "key", "key": "Enter", "ts": 2}),
                json!({"t": "nav", "url": "http://site.test/results?q=rust", "ts": 300}),
                json!({"t": "click", "loc": loc("link", "The Rust Book", "a.r1"), "ts": 2000}),
            ]),
            "Find Book",
        )
        .unwrap();
        let js = to_adapter(&c, "find-book");
        assert!(js.starts_with("/* @meta\n"));
        let meta_end = js.find("*/").unwrap();
        let meta: Value = serde_json::from_str(&js[8..meta_end]).unwrap();
        assert_eq!(meta["name"], "site/find-book");
        assert_eq!(meta["domain"], "site.test");
        assert_eq!(meta["args"]["text1"]["required"], false);
        assert!(js.contains("async function(args)"));
        assert!(!js.contains("dispatchEvent"));
        assert!(!js.contains(".click()"));
        assert!(js.contains("\"navigates\":true"));
    }

    #[test]
    fn panel_save_refuses_a_non_list_program() {
        assert!(panel_handle("teachSave", &json!({"name": "x", "program": {}})).is_err());
        assert!(RECORDER_JS.contains("__cuTeachRecorder"));
        let r = panel_handle(
            "teachCompile",
            &json!({"name": "x", "recording": rec(vec![json!({"t": "key", "key": "Enter", "ts": 1})])}),
        )
        .unwrap();
        assert_eq!(r["schemaVersion"], 1);
        assert!(r["program"].is_array());
    }
}
