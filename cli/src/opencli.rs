//! OpenCLI compatibility: run jackwener/OpenCLI adapters as `site` commands.
//!
//! OpenCLI's adapters run in Node and call a `page` object, so they cannot be
//! evaluated in a tab like our adapters. Instead `site update` installs a pinned
//! `@jackwener/opencli` package (public npm tarball, `--ignore-scripts`) into
//! `~/.chrome-use/opencli`, and `opencli_runner.mjs` runs an adapter with
//! OpenCLI's own registry, argument coercion, pipeline executor and `BasePage`
//! helpers, over a page whose transport is chrome-use. The installed package
//! is used as published; chrome-use writes nothing over it.
//!
//! `site add` also takes OpenCLI-format sources (a local dir such as
//! `~/.opencli/clis`, a GitHub repo, or a zip). They are copied into
//! `~/.chrome-use/opencli/sources/<slug>`, below the package's `node_modules`,
//! so their `@jackwener/opencli/...` imports resolve to the pinned package, and
//! listed into `sources/manifest.json` by the runner's `--list` mode. Remote
//! ones are pinned (commit SHA / zip SHA-256) when the user trusts them, since
//! they run as local Node code; `site update` never moves the pin.
//!
//! Precedence: a chrome-use adapter (official, configured, then community) with
//! the same `name/cmd` always wins; then an added OpenCLI source, then the
//! OpenCLI package. OpenCLI only answers names we don't have, and it is listed
//! last in the domain hint. Commands we rely on are ported to
//! leeguooooo/chrome-use-sites and maintained there (`PORTED`); OpenCLI never
//! answers those names, even before the ported adapter is synced.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// The OpenCLI release we install. Pinned rather than `latest` because its
/// adapters run as local Node code with the user's privileges; a version bump
/// is a deliberate change here. `AGENT_BROWSER_OPENCLI_VERSION` overrides.
pub const PINNED_VERSION: &str = "1.8.8";
pub const PACKAGE: &str = "@jackwener/opencli";
pub const SOURCE_LABEL: &str = "opencli";

const RUNNER_JS: &str = include_str!("opencli_runner.mjs");

/// OpenCLI commands that leeguooooo/chrome-use-sites now maintains, ported
/// from this release under its Apache-2.0 license with attribution. OpenCLI
/// never answers these names: they are dropped from its manifest here, so
/// `site` runs our adapter, and a cache synced before the port arrived is
/// refreshed rather than falling back to the upstream command (main.rs). The
/// ports carry chrome-use's fixes to `douyin/delete` (#508, #525), which used
/// to be written over the installed package.
pub const PORTED: &[&str] = &[
    "douyin/delete",
    "douyin/update",
    "twitter/delete",
    "twitter/like",
    "twitter/follow",
    "bilibili/user-videos",
    "bilibili/creator-stats",
];

/// Whether `spec` is one of the `PORTED` commands.
pub fn is_ported(spec: &str) -> bool {
    PORTED.contains(&spec)
}

pub fn disabled() -> bool {
    std::env::var_os("AGENT_BROWSER_SITES_NO_OPENCLI").is_some()
}

fn version() -> String {
    std::env::var("AGENT_BROWSER_OPENCLI_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| PINNED_VERSION.to_string())
}

/// `~/.chrome-use/opencli` — npm prefix holding the package and the runner.
pub fn root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".chrome-use").join("opencli"))
}

fn pkg_dir(root: &Path) -> PathBuf {
    root.join("node_modules").join("@jackwener").join("opencli")
}

fn installed_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pkg_dir(root).join("package.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

/// npm is `npm.cmd` on Windows; `Command` doesn't resolve PATHEXT for us.
fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// Default ceiling for one OpenCLI command, in seconds. A command that declares
/// its own `timeout` arg (login flows wait for the user) gets that plus a
/// margin. `AGENT_BROWSER_OPENCLI_TIMEOUT` (seconds) overrides the default.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

fn run_timeout(kwargs: &serde_json::Map<String, Value>) -> std::time::Duration {
    let base = std::env::var("AGENT_BROWSER_OPENCLI_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let declared = kwargs
        .get("timeout")
        .and_then(|v| v.as_str())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|t| t + 60)
        .unwrap_or(0);
    std::time::Duration::from_secs(base.max(declared))
}

fn on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Install or update the pinned package. Best-effort: Ok(None) when skipped
/// (disabled, or no node/npm on PATH), Ok(Some(n)) with the command count.
pub fn sync() -> Result<Option<usize>, String> {
    if disabled() {
        return Ok(None);
    }
    if !on_path("node") || !on_path(npm()) {
        return Ok(None);
    }
    let root = root().ok_or("opencli: cannot resolve home dir")?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let want = version();
    if installed_version(&root).as_deref() != Some(want.as_str()) {
        let out = Command::new(npm())
            .arg("install")
            .arg("--prefix")
            .arg(&root)
            .arg(format!("{PACKAGE}@{want}"))
            .args([
                "--omit=dev",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--no-save",
                "--loglevel=error",
            ])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("opencli: npm: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "opencli: npm install {PACKAGE}@{want} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    write_runner(&root)?;
    Ok(Some(manifest().len()))
}

fn write_runner(root: &Path) -> Result<PathBuf, String> {
    let path = root.join("runner.mjs");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(RUNNER_JS) {
        std::fs::write(&path, RUNNER_JS).map_err(|e| format!("opencli: write runner: {e}"))?;
    }
    Ok(path)
}

/// Every OpenCLI command: those from added sources first, then the package's
/// `cli-manifest.json` entries they don't shadow, all without the `PORTED`
/// ones, or empty.
pub fn manifest() -> Vec<Value> {
    if disabled() {
        return Vec::new();
    }
    let Some(root) = root() else {
        return Vec::new();
    };
    let read = |p: PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
            .unwrap_or_default()
    };
    let mut out = read(user_manifest_path(&root));
    let mut seen: std::collections::HashSet<String> = out.iter().filter_map(spec_of).collect();
    for e in read(pkg_dir(&root).join("cli-manifest.json")) {
        if spec_of(&e).is_some_and(|s| seen.insert(s)) {
            out.push(e);
        }
    }
    out.retain(|e| !spec_of(e).is_some_and(|s| is_ported(&s)));
    out
}

pub fn spec_of(entry: &Value) -> Option<String> {
    let site = entry.get("site")?.as_str()?;
    let name = entry.get("name")?.as_str()?;
    Some(format!("{site}/{name}"))
}

/// The manifest entry for `site/name`, if OpenCLI has it.
pub fn lookup(spec: &str) -> Option<Value> {
    manifest()
        .into_iter()
        .find(|e| spec_of(e).as_deref() == Some(spec))
}

/// Whether `spec` should run through OpenCLI: we have no adapter by that name,
/// it is not one we ported, and OpenCLI has it.
pub fn handles(spec: &str) -> bool {
    spec.contains('/')
        && !is_ported(spec)
        && crate::site::load_adapter(spec).is_err()
        && lookup(spec).is_some()
}

/// `site info` for an OpenCLI command, shaped like an adapter's @meta.
pub fn info(entry: &Value) -> Value {
    let mut args = serde_json::Map::new();
    for a in entry
        .get("args")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if let Some(name) = a.get("name").and_then(|v| v.as_str()) {
            args.insert(name.to_string(), a.clone());
        }
    }
    json!({
        "name": spec_of(entry),
        "description": entry.get("description"),
        "domain": entry.get("domain"),
        "readOnly": entry.get("access").and_then(|v| v.as_str()) == Some("read"),
        "strategy": entry.get("strategy"),
        "args": args,
        "columns": entry.get("columns"),
        "source": source_label(entry),
        "maintainedBy": SOURCE_LABEL,
    })
}

/// Where a command comes from: the added source it was listed from, else the
/// pinned package.
fn source_label(entry: &Value) -> String {
    match origin(entry) {
        Some(o) => format!("{SOURCE_LABEL} ({o})"),
        None => format!("{SOURCE_LABEL} ({PACKAGE}@{})", version()),
    }
}

/// The `site add` source an entry was listed from, if any.
pub fn origin(entry: &Value) -> Option<&str> {
    entry.get("origin").and_then(|v| v.as_str())
}

/// Whether OpenCLI declares `entry` read-only. Anything else (a write, or an
/// entry with no `access`) counts as a write.
pub fn is_read(entry: &Value) -> bool {
    entry.get("access").and_then(|v| v.as_str()) == Some("read")
}

/// The stderr note printed whenever a command runs through OpenCLI: it is
/// third-party code chrome-use does not maintain (`douyin/delete` and
/// `twitter/delete` shipped broken before being ported), so a write on the
/// user's account should move to an adapter of ours.
pub fn third_party_note(spec: &str, entry: &Value) -> String {
    let base = match origin(entry) {
        Some(o) => format!(
            "note: site {spec} is an OpenCLI command from {o}, which you added: it runs as Node code \
             with your user's permissions and is not maintained by chrome-use"
        ),
        None => format!(
            "note: site {spec} is a third-party OpenCLI command, not maintained by chrome-use"
        ),
    };
    if is_read(entry) {
        format!("{base}; it may break without notice")
    } else {
        format!(
            "{base}. It acts on the user's account: our own adapter is recommended. \
             Offer the user to port it into chrome-use-sites rather than rely on it"
        )
    }
}

/// The `--json` data for a result OpenCLI produced: the result plus who
/// maintains it, whether it was a read, and the same note as stderr.
pub fn tagged_data(spec: &str, entry: &Value, result: Value) -> Value {
    json!({
        "result": result,
        "source": SOURCE_LABEL,
        "maintainedBy": SOURCE_LABEL,
        "origin": origin(entry),
        "readOnly": is_read(entry),
        "notice": third_party_note(spec, entry),
    })
}

/// Map CLI args onto the command's declared args: positional args fill the
/// ones marked `positional` in order, `--key value` sets by name, and a bare
/// `--flag` sets a boolean arg. Values stay strings; OpenCLI coerces them.
pub fn map_args(entry: &Value, rest: &[String]) -> Result<serde_json::Map<String, Value>, String> {
    let declared: Vec<&Value> = entry
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let positional: Vec<&str> = declared
        .iter()
        .filter(|a| a.get("positional").and_then(|v| v.as_bool()) == Some(true))
        .filter_map(|a| a.get("name").and_then(|v| v.as_str()))
        .collect();
    let is_bool = |name: &str| {
        declared.iter().any(|a| {
            a.get("name").and_then(|v| v.as_str()) == Some(name)
                && matches!(
                    a.get("type").and_then(|v| v.as_str()),
                    Some("bool") | Some("boolean")
                )
        })
    };
    let mut out = serde_json::Map::new();
    let mut pos = positional.iter();
    let mut it = rest.iter().peekable();
    while let Some(a) = it.next() {
        if let Some(key) = a.strip_prefix("--") {
            let (key, inline) = match key.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (key, None),
            };
            let value = match inline {
                Some(v) => v,
                None if is_bool(key) && it.peek().is_none_or(|n| n.starts_with("--")) => {
                    "true".to_string()
                }
                None => it
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("site: --{key} needs a value"))?,
            };
            out.insert(key.to_string(), Value::String(value));
        } else {
            let name = pos.next().ok_or_else(|| {
                format!(
                    "site: unexpected argument `{a}` (positional args: {})",
                    if positional.is_empty() {
                        "none".to_string()
                    } else {
                        positional.join(", ")
                    }
                )
            })?;
            out.insert(name.to_string(), Value::String(a.clone()));
        }
    }
    Ok(out)
}

/// Same-meaning arg names across packs, for a fallback from one of our
/// adapters to OpenCLI's command of the same name.
const ARG_ALIASES: &[&[&str]] = &[
    &["limit", "count", "n", "num", "max", "size"],
    &["query", "q", "keyword", "keywords", "term"],
    &["id", "item", "itemid"],
    &["user", "username", "uid", "userid"],
];

/// Turn our adapter's resolved args (`{name: value}`) into `--name value`
/// pairs OpenCLI's command declares, renaming through ARG_ALIASES and dropping
/// what it doesn't take.
pub fn fallback_args(entry: &Value, ours: &Value) -> Vec<String> {
    let declared: Vec<String> = entry
        .get("args")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|a| {
            a.get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_ascii_lowercase())
        })
        .collect();
    let mut out = Vec::new();
    for (k, v) in ours.as_object().into_iter().flatten() {
        let value = match v {
            Value::String(s) => s.clone(),
            Value::Null => continue,
            other => other.to_string(),
        };
        let key = k.to_ascii_lowercase();
        let target = if declared.contains(&key) {
            Some(key)
        } else {
            ARG_ALIASES
                .iter()
                .find(|group| group.contains(&key.as_str()))
                .and_then(|group| group.iter().find(|n| declared.iter().any(|d| d == *n)))
                .map(|n| n.to_string())
        };
        if let Some(t) = target {
            out.push(format!("--{t}"));
            out.push(value);
        }
    }
    out
}

/// Wait for `child`, killing it at `limit`. Its piped stdout and stderr are
/// read on threads so a chatty command can't fill a pipe while we wait.
/// Returns (stdout, stderr, timed out).
fn wait_with_deadline(
    mut child: std::process::Child,
    limit: std::time::Duration,
) -> (String, String, bool) {
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let started = std::time::Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => break false,
        }
    };
    (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
        timed_out,
    )
}

/// Ceiling for listing one added source (importing its modules).
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Run `site/name` through OpenCLI's runtime. Returns the runner's envelope:
/// `{success, data}` or `{success: false, error, hint?}`.
pub fn run(spec: &str, entry: &Value, rest: &[String], session: &str) -> Value {
    let fail = |e: String| json!({ "success": false, "error": e });
    if !on_path("node") {
        return fail(format!(
            "site {spec} comes from OpenCLI, which needs Node.js 20+ on PATH (https://nodejs.org)"
        ));
    }
    let Some(root) = root() else {
        return fail("opencli: cannot resolve home dir".into());
    };
    let runner = match write_runner(&root) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let kwargs = match map_args(entry, rest) {
        Ok(k) => k,
        Err(e) => return fail(e),
    };
    let (site, name) = spec.split_once('/').unwrap_or((spec, ""));
    let chrome_use = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "chrome-use".to_string());
    let req = json!({
        "pkgDir": pkg_dir(&root),
        "clisDir": entry.get("clisDir"),
        "chromeUse": chrome_use,
        "session": session,
        "site": site,
        "name": name,
        "modulePath": entry.get("modulePath"),
        "kwargs": kwargs,
    });
    let req_path =
        std::env::temp_dir().join(format!("cu-opencli-{}.json", uuid::Uuid::new_v4().simple()));
    if let Err(e) = std::fs::write(&req_path, req.to_string()) {
        return fail(format!("opencli: write request: {e}"));
    }
    let limit = run_timeout(&kwargs);
    let child = Command::new("node")
        .arg(&runner)
        .arg(&req_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&req_path);
            return fail(format!("opencli: node: {e}"));
        }
    };
    let (stdout, _, timed_out) = wait_with_deadline(child, limit);
    let _ = std::fs::remove_file(&req_path);
    if timed_out {
        return fail(format!(
            "site {spec} (OpenCLI) did not finish within {}s; the page may still be working. \
             Set AGENT_BROWSER_OPENCLI_TIMEOUT=<seconds> for a longer limit",
            limit.as_secs()
        ));
    }
    stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .unwrap_or_else(|| fail(format!("opencli: {spec} produced no result")))
}

/// `~/.chrome-use/opencli.sources` — OpenCLI-format sources added with
/// `site add`, one per line: `<source>` for a local dir, `<source> @<pin>`
/// for a remote one (commit SHA or zip SHA-256). Kept apart from
/// `sites.sources` so older chrome-use versions never sync them as packs of ours.
pub fn sources_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".chrome-use").join("opencli.sources"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedSource {
    pub source: String,
    pub pin: Option<String>,
}

fn parse_source_line(line: &str) -> Option<AddedSource> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    Some(match line.rsplit_once(" @") {
        Some((src, pin)) if !pin.is_empty() && !pin.contains(' ') => AddedSource {
            source: src.trim().to_string(),
            pin: Some(pin.to_string()),
        },
        _ => AddedSource {
            source: line.to_string(),
            pin: None,
        },
    })
}

pub fn read_sources() -> Vec<AddedSource> {
    sources_config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().filter_map(parse_source_line).collect())
        .unwrap_or_default()
}

fn write_sources(list: &[AddedSource]) -> Result<(), String> {
    let path = sources_config_path().ok_or("opencli: cannot resolve home dir")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text: String = list
        .iter()
        .map(|s| match &s.pin {
            Some(p) => format!("{} @{p}\n", s.source),
            None => format!("{}\n", s.source),
        })
        .collect();
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Add `source` at `pin`, replacing an earlier entry for the same source.
pub fn set_source(source: &str, pin: Option<&str>) -> Result<(), String> {
    let mut list = read_sources();
    let entry = AddedSource {
        source: source.to_string(),
        pin: pin.map(String::from),
    };
    match list.iter_mut().find(|s| s.source == source) {
        Some(s) => *s = entry,
        None => list.push(entry),
    }
    write_sources(&list)
}

/// Remove `source` and its installed copy, and re-list the rest. Returns
/// whether it was configured.
pub fn remove_source(source: &str) -> Result<bool, String> {
    let mut list = read_sources();
    let before = list.len();
    list.retain(|s| s.source != source);
    if list.len() == before {
        return Ok(false);
    }
    write_sources(&list)?;
    if let Some(dir) = sources_root().map(|r| r.join(slug(source))) {
        let _ = std::fs::remove_dir_all(dir);
    }
    let _ = index_sources();
    Ok(true)
}

/// `~/.chrome-use/opencli/sources` — installed copies of the added sources.
pub fn sources_root() -> Option<PathBuf> {
    root().map(|r| r.join("sources"))
}

fn user_manifest_path(root: &Path) -> PathBuf {
    root.join("sources").join("manifest.json")
}

/// A directory name for `source`: readable, plus a hash so two sources that
/// sanitize alike stay apart.
pub fn slug(source: &str) -> String {
    let readable: String = source
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let tail: String = readable
        .chars()
        .rev()
        .take(40)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(source.as_bytes()));
    format!("{tail}-{}", &digest[..8])
}

/// Whether the OpenCLI package is installed (needed to list or run a source).
pub fn installed() -> bool {
    root().is_some_and(|r| installed_version(&r).is_some())
}

/// A fresh staging directory under `sources/`, where a fetched source can
/// resolve the package's modules. Remove it or `install_staged` it.
pub fn staging_dir() -> Result<PathBuf, String> {
    let root = sources_root().ok_or("opencli: cannot resolve home dir")?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    // ESM: OpenCLI adapters are `.js` modules with `import` statements.
    let pkg = root.join("package.json");
    if !pkg.exists() {
        std::fs::write(&pkg, "{\"private\": true, \"type\": \"module\"}\n")
            .map_err(|e| e.to_string())?;
    }
    Ok(root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple())))
}

/// Replace the installed copy of `source` with `staged`, recording the pin.
pub fn install_staged(source: &str, pin: Option<&str>, staged: &Path) -> Result<PathBuf, String> {
    let dest = sources_root()
        .ok_or("opencli: cannot resolve home dir")?
        .join(slug(source));
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(staged, &dest).map_err(|e| format!("opencli: install {source}: {e}"))?;
    if let Some(p) = pin {
        let _ = std::fs::write(dest.join(".chrome-use-pin"), p);
    }
    Ok(dest)
}

/// The pin the installed copy of `source` was fetched at.
pub fn installed_pin(source: &str) -> Option<String> {
    let dir = sources_root()?.join(slug(source));
    std::fs::read_to_string(dir.join(".chrome-use-pin"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// The adapter modules under `dir`, relative, without running them — what
/// `site add` shows before the user trusts a source.
pub fn adapter_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&cur) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            // A .ts counts unless a compiled .js sits beside it.
            let module = (name.ends_with(".js") && !name.ends_with(".test.js"))
                || (name.ends_with(".ts")
                    && !name.ends_with(".d.ts")
                    && !name.ends_with(".test.ts")
                    && !p.with_extension("js").exists());
            if p.is_dir() {
                stack.push(p);
            } else if module
                && std::fs::read_to_string(&p).is_ok_and(|t| t.contains("@jackwener/opencli"))
            {
                if let Ok(rel) = p.strip_prefix(dir) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    out
}

/// npm dependencies a source declares. They are not installed: an adapter
/// importing one fails to load, and `site add` says so up front.
pub fn declared_dependencies(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let mut deps: Vec<String> = v
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|o| {
            o.keys()
                .filter(|k| k.as_str() != PACKAGE)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    deps.sort();
    deps
}

/// Run the runner's `--list` mode over `dir`: the commands its modules
/// register, plus per-file load errors. This imports (runs) the modules.
pub fn list_dir(dir: &Path) -> Result<(Vec<Value>, Vec<String>), String> {
    let root = root().ok_or("opencli: cannot resolve home dir")?;
    let runner = write_runner(&root)?;
    let req = json!({ "pkgDir": pkg_dir(&root), "clisDir": dir });
    let req_path =
        std::env::temp_dir().join(format!("cu-opencli-{}.json", uuid::Uuid::new_v4().simple()));
    std::fs::write(&req_path, req.to_string()).map_err(|e| format!("opencli: {e}"))?;
    let child = Command::new("node")
        .arg(&runner)
        .arg("--list")
        .arg(&req_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&req_path);
            return Err(format!("opencli: node: {e}"));
        }
    };
    let (stdout, stderr, timed_out) = wait_with_deadline(child, LIST_TIMEOUT);
    let _ = std::fs::remove_file(&req_path);
    if timed_out {
        return Err(format!(
            "opencli: listing {} did not finish within {}s (a module may wait forever while it loads)",
            dir.display(),
            LIST_TIMEOUT.as_secs()
        ));
    }
    let env = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .ok_or_else(|| {
            format!(
                "opencli: listing {} failed: {}",
                dir.display(),
                stderr.trim()
            )
        })?;
    if env.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return Err(format!(
            "opencli: listing {} failed: {}",
            dir.display(),
            env.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
        ));
    }
    let entries = env
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let errors = env
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok((entries, errors))
}

/// What listing one added source found.
#[derive(Debug, Default)]
pub struct SourceReport {
    pub source: String,
    /// Every command its modules register.
    pub commands: Vec<String>,
    /// Commands an earlier added source already provides: (spec, that source).
    pub taken: Vec<(String, String)>,
    /// Modules that failed to load, or a failure to list the source at all.
    pub problems: Vec<String>,
}

/// List every installed added source into `sources/manifest.json`, each entry
/// tagged with its `clisDir` and `origin`. Earlier sources win a shared name.
pub fn index_sources() -> Result<Vec<SourceReport>, String> {
    let root = root().ok_or("opencli: cannot resolve home dir")?;
    let mut all: Vec<Value> = Vec::new();
    let mut owner: std::collections::HashMap<String, String> = Default::default();
    let mut reports = Vec::new();
    if installed() {
        for s in read_sources() {
            let dir = root.join("sources").join(slug(&s.source));
            if !dir.is_dir() {
                continue;
            }
            let mut report = SourceReport {
                source: s.source.clone(),
                ..Default::default()
            };
            match list_dir(&dir) {
                Ok((entries, errors)) => {
                    report.problems = errors;
                    for mut e in entries {
                        let Some(spec) = spec_of(&e) else {
                            continue;
                        };
                        report.commands.push(spec.clone());
                        if let Some(by) = owner.get(&spec) {
                            report.taken.push((spec, by.clone()));
                            continue;
                        }
                        owner.insert(spec, s.source.clone());
                        e["clisDir"] = json!(dir);
                        e["origin"] = json!(s.source);
                        all.push(e);
                    }
                }
                Err(e) => report.problems.push(e),
            }
            reports.push(report);
        }
    }
    let path = user_manifest_path(&root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string(&all).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Value {
        json!({
            "site": "bilibili", "name": "feed", "domain": "www.bilibili.com", "access": "read",
            "args": [
                {"name": "uid", "positional": true},
                {"name": "limit", "type": "int"},
                {"name": "verbose", "type": "bool"}
            ]
        })
    }

    #[test]
    fn maps_positional_named_and_bare_bool_args() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let m = map_args(&entry(), &s(&["123", "--limit", "5", "--verbose"])).unwrap();
        assert_eq!(m["uid"], "123");
        assert_eq!(m["limit"], "5");
        assert_eq!(m["verbose"], "true");
        let m = map_args(&entry(), &s(&["--limit=7"])).unwrap();
        assert_eq!(m["limit"], "7");
        assert!(map_args(&entry(), &s(&["a", "b"])).is_err());
        assert!(map_args(&entry(), &s(&["--limit"])).is_err());
    }

    #[test]
    fn timeout_defaults_and_follows_a_declared_timeout_arg() {
        let mut k = serde_json::Map::new();
        assert_eq!(run_timeout(&k).as_secs(), DEFAULT_TIMEOUT_SECS);
        k.insert("timeout".into(), json!("600"));
        assert_eq!(run_timeout(&k).as_secs(), 660);
        k.insert("timeout".into(), json!("10"));
        assert_eq!(run_timeout(&k).as_secs(), DEFAULT_TIMEOUT_SECS);
    }

    #[test]
    fn fallback_args_rename_through_aliases_and_drop_unknown() {
        let theirs = json!({"args": [{"name": "limit"}, {"name": "query"}]});
        let ours = json!({"count": 3, "q": "rust", "sort": "new", "x": null});
        let a = fallback_args(&theirs, &ours);
        let pairs: Vec<_> = a
            .chunks(2)
            .map(|c| (c[0].as_str(), c[1].as_str()))
            .collect();
        assert!(pairs.contains(&("--limit", "3")), "{a:?}");
        assert!(pairs.contains(&("--query", "rust")), "{a:?}");
        assert_eq!(pairs.len(), 2, "{a:?}");
    }

    /// A HOME with an OpenCLI manifest listing `opencli` and adapters of ours
    /// at `ours` (each `name/cmd`).
    fn home_with(opencli: &[&str], ours: &[&str]) -> PathBuf {
        let home =
            std::env::temp_dir().join(format!("cu-oc-prec-{}", uuid::Uuid::new_v4().simple()));
        let pkg = pkg_dir(&home.join(".chrome-use").join("opencli"));
        std::fs::create_dir_all(&pkg).unwrap();
        let entries: Vec<Value> = opencli
            .iter()
            .map(|s| {
                let (site, name) = s.split_once('/').unwrap();
                json!({"site": site, "name": name, "access": "write", "domain": "example.com"})
            })
            .collect();
        std::fs::write(
            pkg.join("cli-manifest.json"),
            serde_json::to_string(&entries).unwrap(),
        )
        .unwrap();
        for spec in ours {
            let (name, cmd) = spec.split_once('/').unwrap();
            let dir = home.join(".chrome-use").join("sites").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let meta = json!({"name": spec, "domain": "example.com", "args": {}});
            std::fs::write(
                dir.join(format!("{cmd}.js")),
                format!("/* @meta\n{meta}\n*/\nasync function(args) {{ return {{}} }}\n"),
            )
            .unwrap();
        }
        home
    }

    #[test]
    fn our_adapter_wins_over_opencli_for_the_same_name() {
        let guard = crate::test_utils::EnvGuard::new(&["HOME", "AGENT_BROWSER_SITES_NO_OPENCLI"]);
        guard.remove("AGENT_BROWSER_SITES_NO_OPENCLI");
        let home = home_with(
            &[
                "bilibili/feed",
                "hackernews/best",
                "douyin/delete",
                "douyin/update",
                "twitter/delete",
            ],
            &["bilibili/feed", "douyin/delete"],
        );
        guard.set("HOME", home.to_str().unwrap());

        // Both have it: ours.
        assert!(crate::site::load_adapter("bilibili/feed").is_ok());
        assert!(lookup("bilibili/feed").is_some());
        assert!(!handles("bilibili/feed"));
        // Only OpenCLI has it: OpenCLI.
        assert!(handles("hackernews/best"));
        // Ported and synced: ours.
        assert!(crate::site::load_adapter("douyin/delete").is_ok());
        assert!(!handles("douyin/delete"));
        // Ported but not synced yet: still never OpenCLI.
        assert!(crate::site::load_adapter("douyin/update").is_err());
        assert!(!handles("douyin/update"));
        assert!(!handles("twitter/delete"));
        for spec in PORTED {
            assert!(lookup(spec).is_none(), "{spec} must not resolve to OpenCLI");
        }
        let listed: Vec<String> = manifest().iter().filter_map(spec_of).collect();
        assert_eq!(listed, ["bilibili/feed", "hackernews/best"]);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn ported_names_cover_the_douyin_writes() {
        for spec in [
            "douyin/delete",
            "douyin/update",
            "twitter/delete",
            "twitter/like",
            "twitter/follow",
            "bilibili/user-videos",
            "bilibili/creator-stats",
        ] {
            assert!(is_ported(spec), "{spec}");
        }
        assert!(!is_ported("douyin/videos"));
    }

    #[test]
    fn info_reads_like_adapter_meta() {
        let i = info(&entry());
        assert_eq!(i["name"], "bilibili/feed");
        assert_eq!(i["readOnly"], true);
        assert!(i["args"].get("limit").is_some());
        assert!(i["source"].as_str().unwrap().starts_with("opencli"));
        assert_eq!(i["maintainedBy"], "opencli");
    }

    #[test]
    fn note_flags_third_party_and_steers_writes_to_a_port() {
        let read = third_party_note("bilibili/feed", &entry());
        assert!(read.contains("third-party OpenCLI command, not maintained by chrome-use"));
        assert!(!read.contains("chrome-use-sites"), "{read}");

        let write = json!({"site": "twitter", "name": "like", "access": "write"});
        let w = third_party_note("twitter/like", &write);
        assert!(w.contains("not maintained by chrome-use"), "{w}");
        assert!(w.contains("our own adapter is recommended"), "{w}");
        assert!(w.contains("port it into chrome-use-sites"), "{w}");
        // No declared access is treated as a write.
        assert!(third_party_note("x/y", &Value::Null).contains("chrome-use-sites"));
        assert!(!w.contains('\n'), "one line: {w}");
    }

    #[test]
    fn json_data_marks_opencli_as_maintainer() {
        let d = tagged_data("bilibili/feed", &entry(), json!([1]));
        assert_eq!(d["result"], json!([1]));
        assert_eq!(d["source"], "opencli");
        assert_eq!(d["maintainedBy"], "opencli");
        assert_eq!(d["readOnly"], true);
        assert_eq!(d["notice"], third_party_note("bilibili/feed", &entry()));

        let write = json!({"access": "write"});
        let d = tagged_data("twitter/like", &write, Value::Null);
        assert_eq!(d["readOnly"], false);
        assert!(d["notice"].as_str().unwrap().contains("chrome-use-sites"));
    }
    #[test]
    fn source_lines_round_trip_with_and_without_a_pin() {
        assert_eq!(parse_source_line("  # c "), None);
        assert_eq!(
            parse_source_line("/Users/me/my clis"),
            Some(AddedSource {
                source: "/Users/me/my clis".into(),
                pin: None
            })
        );
        assert_eq!(
            parse_source_line("me/repo @0123abcd"),
            Some(AddedSource {
                source: "me/repo".into(),
                pin: Some("0123abcd".into())
            })
        );

        let guard = crate::test_utils::EnvGuard::new(&["HOME"]);
        let home =
            std::env::temp_dir().join(format!("cu-oc-src-{}", uuid::Uuid::new_v4().simple()));
        guard.set("HOME", home.to_str().unwrap());
        set_source("me/repo", Some("aaa")).unwrap();
        set_source("/tmp/local clis", None).unwrap();
        set_source("me/repo", Some("bbb")).unwrap();
        assert_eq!(
            read_sources(),
            vec![
                AddedSource {
                    source: "me/repo".into(),
                    pin: Some("bbb".into())
                },
                AddedSource {
                    source: "/tmp/local clis".into(),
                    pin: None
                },
            ]
        );
        assert!(remove_source("me/repo").unwrap());
        assert!(!remove_source("me/repo").unwrap());
        assert_eq!(read_sources().len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn slugs_are_readable_and_distinct() {
        let a = slug("/Users/me/.opencli/clis");
        assert!(a.starts_with("users-me-opencli-clis-"), "{a}");
        assert_ne!(slug("me/repo"), slug("me-repo"));
        assert!(!slug("https://x.y/a.zip").contains('/'));
    }

    #[test]
    fn added_sources_shadow_the_package_but_not_ours_or_ported() {
        let guard = crate::test_utils::EnvGuard::new(&["HOME", "AGENT_BROWSER_SITES_NO_OPENCLI"]);
        guard.remove("AGENT_BROWSER_SITES_NO_OPENCLI");
        let home = home_with(&["hackernews/best", "bilibili/feed"], &["bilibili/feed"]);
        guard.set("HOME", home.to_str().unwrap());
        let root = home.join(".chrome-use").join("opencli");
        let user = json!([
            {"site": "hackernews", "name": "best", "access": "read", "origin": "me/repo", "clisDir": "/x"},
            {"site": "bilibili", "name": "feed", "access": "read", "origin": "me/repo", "clisDir": "/x"},
            {"site": "douyin", "name": "delete", "access": "write", "origin": "me/repo", "clisDir": "/x"},
            {"site": "mine", "name": "thing", "access": "write", "origin": "me/repo", "clisDir": "/x"}
        ]);
        std::fs::create_dir_all(root.join("sources")).unwrap();
        std::fs::write(user_manifest_path(&root), user.to_string()).unwrap();

        let best = lookup("hackernews/best").unwrap();
        assert_eq!(
            origin(&best),
            Some("me/repo"),
            "added source wins over the package"
        );
        assert_eq!(
            manifest()
                .iter()
                .filter(|e| spec_of(e).as_deref() == Some("hackernews/best"))
                .count(),
            1
        );
        assert!(handles("mine/thing"));
        assert!(!handles("bilibili/feed"), "our adapter still wins");
        assert!(
            lookup("douyin/delete").is_none(),
            "ported names never go to OpenCLI"
        );

        let info = info(&best);
        assert_eq!(info["source"], "opencli (me/repo)");
        let note = third_party_note("mine/thing", &lookup("mine/thing").unwrap());
        assert!(note.contains("from me/repo, which you added"), "{note}");
        assert!(note.contains("Node code"), "{note}");
        assert!(
            note.contains("chrome-use-sites"),
            "writes still steer to a port: {note}"
        );
        assert_eq!(
            tagged_data("mine/thing", &lookup("mine/thing").unwrap(), Value::Null)["origin"],
            "me/repo"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn adapter_files_and_dependencies_are_read_without_running_code() {
        let dir =
            std::env::temp_dir().join(format!("cu-oc-files-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("hn")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/x")).unwrap();
        let module = "import { cli } from '@jackwener/opencli/registry';\ncli({});\n";
        std::fs::write(dir.join("hn/top.js"), module).unwrap();
        std::fs::write(dir.join("hn/top.test.js"), module).unwrap();
        std::fs::write(dir.join("hn/top.ts"), module).unwrap();
        std::fs::write(dir.join("hn/new.ts"), module).unwrap();
        std::fs::write(dir.join("hn/util.js"), "export const x = 1;\n").unwrap();
        std::fs::write(dir.join("node_modules/x/index.js"), module).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"type":"module","dependencies":{"@jackwener/opencli":"^1","cheerio":"1"}}"#,
        )
        .unwrap();
        assert_eq!(adapter_files(&dir), ["hn/new.ts", "hn/top.js"]);
        assert_eq!(declared_dependencies(&dir), ["cheerio"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[cfg(unix)]
    #[test]
    fn wait_with_deadline_collects_output_and_kills_a_hung_child() {
        let spawn = |script: &str| {
            Command::new("sh")
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        };
        let (out, err, timed_out) = wait_with_deadline(
            spawn("echo out; echo err >&2"),
            std::time::Duration::from_secs(10),
        );
        assert_eq!((out.trim(), err.trim(), timed_out), ("out", "err", false));

        let started = std::time::Instant::now();
        let (_, _, timed_out) = wait_with_deadline(
            spawn("exec sleep 30"),
            std::time::Duration::from_millis(300),
        );
        assert!(timed_out);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }
}
