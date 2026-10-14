//! `.chrome-use/inspect.json`: a team-committed file that names the origins
//! and path scopes an app is served under. It proposes a target; it never
//! authorizes one. Only files tracked by git inside a known worktree are read,
//! never through a symlink, and anything outside the strict schema rejects the
//! whole file with a reason the frame can show.
use super::scope::{self, OriginPattern};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub(super) const MAX_CONFIG: u64 = 64 * 1024;
const MAX_SITES: usize = 32;
const MAX_ORIGINS: usize = 16;
const MAX_SOURCE_ROOTS: usize = 8;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct File {
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    source_roots: Vec<String>,
    sites: Vec<Site>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Site {
    origins: Vec<String>,
    #[serde(default)]
    path_scope: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct ConfigSite {
    pub origins: Vec<OriginPattern>,
    pub path_scope: String,
}

#[derive(Clone, Debug)]
pub(super) struct Config {
    /// The file itself (inside `toplevel`).
    pub path: PathBuf,
    /// The parent of the `.chrome-use` directory.
    pub app_root: PathBuf,
    /// `app_root` first, then the file's extra roots, all canonical and
    /// inside `toplevel`.
    pub source_roots: Vec<PathBuf>,
    pub sites: Vec<ConfigSite>,
}

/// A site of this config that serves `origin` at `page`: the longest
/// matching `pathScope`, and whether only a wildcard origin matched.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SiteMatch {
    pub path_scope: String,
    pub wildcard: bool,
}

impl Config {
    pub(super) fn site_for(&self, origin: &str, page: &str) -> Option<SiteMatch> {
        self.sites
            .iter()
            .filter(|s| scope::applies(&s.path_scope, page))
            .filter_map(|s| {
                // An exact origin in a site outranks a wildcard in it.
                let wildcard = s.origins.iter().filter_map(|o| o.matches(origin)).min()?;
                Some(SiteMatch {
                    path_scope: s.path_scope.clone(),
                    wildcard,
                })
            })
            .max_by(|a, b| {
                a.path_scope
                    .len()
                    .cmp(&b.path_scope.len())
                    .then(b.wildcard.cmp(&a.wildcard))
            })
    }

    /// The site serving `origin` with exactly `path_scope`, if any.
    pub(super) fn site_with_scope(&self, origin: &str, path_scope: &str) -> Option<SiteMatch> {
        self.sites
            .iter()
            .filter(|s| s.path_scope == path_scope)
            .filter_map(|s| {
                let wildcard = s.origins.iter().filter_map(|o| o.matches(origin)).min()?;
                Some(SiteMatch {
                    path_scope: s.path_scope.clone(),
                    wildcard,
                })
            })
            .min_by_key(|m| m.wildcard)
    }
}

/// `git ls-files` output (NUL separated, `/` separators) reduced to the
/// entries whose last two components are exactly `.chrome-use/inspect.json`.
pub(super) fn listed(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let mut parts = l.rsplit('/');
            parts.next() == Some("inspect.json") && parts.next() == Some(".chrome-use")
        })
        .map(str::to_owned)
        .collect()
}

/// Read and validate the config `rel` (as `git ls-files` printed it) inside
/// the canonical worktree `toplevel`.
pub(super) fn load(toplevel: &Path, rel: &str) -> Result<Config, String> {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() < 2
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('\\'))
        || !Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err("path is not a plain path inside the worktree".into());
    }
    // Neither the file nor any directory between the worktree and it may be
    // a symlink.
    let mut path = toplevel.to_path_buf();
    for part in &parts {
        path.push(part);
        let meta = fs::symlink_metadata(&path).map_err(|_| "file is missing or unreadable")?;
        if meta.file_type().is_symlink() {
            return Err("a symlink is not read".into());
        }
    }
    let meta = fs::symlink_metadata(&path).map_err(|_| "file is unreadable")?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_CONFIG {
        return Err(format!("larger than {} KiB", MAX_CONFIG / 1024));
    }
    let mut data = Vec::new();
    open_nofollow(&path)?
        .take(MAX_CONFIG + 1)
        .read_to_end(&mut data)
        .map_err(|_| "file is unreadable")?;
    if data.len() as u64 > MAX_CONFIG {
        return Err(format!("larger than {} KiB", MAX_CONFIG / 1024));
    }
    let value: Value = serde_json::from_slice(&data).map_err(|e| format!("invalid JSON: {e}"))?;
    match value.get("version").and_then(Value::as_u64) {
        Some(1) => {}
        Some(v) => return Err(format!("unsupported version {v}; expected 1")),
        None => return Err("missing \"version\": 1".into()),
    }
    let file: File = serde_json::from_value(value).map_err(|e| format!("invalid schema: {e}"))?;
    if file.sites.is_empty() || file.sites.len() > MAX_SITES {
        return Err(format!("expected 1 through {MAX_SITES} sites"));
    }
    if file.source_roots.len() > MAX_SOURCE_ROOTS {
        return Err(format!("at most {MAX_SOURCE_ROOTS} sourceRoots"));
    }
    let mut sites = Vec::new();
    for site in file.sites {
        if site.origins.is_empty() || site.origins.len() > MAX_ORIGINS {
            return Err(format!("each site needs 1 through {MAX_ORIGINS} origins"));
        }
        let origins = site
            .origins
            .iter()
            .map(|o| OriginPattern::parse(o))
            .collect::<Result<Vec<_>, _>>()?;
        let path_scope = scope::path_scope(site.path_scope.as_deref().unwrap_or("/"))?;
        sites.push(ConfigSite {
            origins,
            path_scope,
        });
    }
    // `<app>/.chrome-use/inspect.json` → `<app>`.
    let app_rel: Vec<&str> = parts[..parts.len() - 2].to_vec();
    let app_root = app_rel
        .iter()
        .fold(toplevel.to_path_buf(), |p, c| p.join(c));
    let mut source_roots = vec![app_root.clone()];
    for raw in &file.source_roots {
        let root = source_root(toplevel, &app_rel, raw)?;
        if !source_roots.contains(&root) {
            source_roots.push(root);
        }
    }
    Ok(Config {
        path,
        app_root,
        source_roots,
        sites,
    })
}

/// A `sourceRoots` entry, relative to the app root. `..` may climb toward the
/// worktree toplevel but never past it, and the canonical directory must
/// still be inside the worktree.
fn source_root(toplevel: &Path, app_rel: &[&str], raw: &str) -> Result<PathBuf, String> {
    let bad = |why: &str| format!("sourceRoots entry {raw:?} {why}");
    if raw.is_empty() || raw.len() > 300 || raw.chars().any(char::is_control) {
        return Err(bad("is not a plain relative path"));
    }
    if raw.starts_with(['/', '\\']) || Path::new(raw).is_absolute() || raw.contains(':') {
        return Err(bad("must be relative to the app root"));
    }
    let mut stack: Vec<&str> = app_rel.to_vec();
    for part in raw.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Err(bad("escapes the worktree"));
                }
            }
            p => stack.push(p),
        }
    }
    let lexical = stack.iter().fold(toplevel.to_path_buf(), |p, c| p.join(c));
    let canonical = fs::canonicalize(&lexical)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| bad("is not an existing directory"))?;
    if !canonical.starts_with(toplevel) {
        return Err(bad("resolves outside the worktree"));
    }
    Ok(canonical)
}

fn open_nofollow(path: &Path) -> Result<fs::File, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    super::super::windows::open_options(&mut options, false);
    let file = options.open(path).map_err(|_| "file is unreadable")?;
    #[cfg(windows)]
    super::super::windows::regular(&file).map_err(|_| "not a regular file")?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn worktree() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let top = fs::canonicalize(temp.path()).unwrap().join("repo");
        fs::create_dir_all(top.join("apps/admin/.chrome-use")).unwrap();
        fs::create_dir_all(top.join("packages/ui")).unwrap();
        (temp, top)
    }

    fn write(top: &Path, rel: &str, body: &str) {
        let path = top.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    const REL: &str = "apps/admin/.chrome-use/inspect.json";

    #[test]
    fn ls_files_output_keeps_only_exact_config_names() {
        let out = ".chrome-use/inspect.json\0apps/a/.chrome-use/inspect.json\0x.chrome-use/inspect.json\0apps/b/inspect.json\0apps/c/.chrome-use/sub/inspect.json\0";
        assert_eq!(
            listed(out),
            [
                ".chrome-use/inspect.json",
                "apps/a/.chrome-use/inspect.json"
            ]
        );
    }

    #[test]
    fn a_valid_config_names_its_app_root_and_source_roots() {
        let (_t, top) = worktree();
        write(
            &top,
            REL,
            r#"{"version":1,"sourceRoots":["../../packages/ui"],"sites":[
                {"origins":["https://test-admin.x.com","http://localhost:5173"]},
                {"origins":["https://*.preview.x.com"],"pathScope":"/admin"}]}"#,
        );
        let c = load(&top, REL).unwrap();
        assert_eq!(c.app_root, top.join("apps/admin"));
        assert_eq!(
            c.source_roots,
            [top.join("apps/admin"), top.join("packages/ui")]
        );
        assert_eq!(
            c.site_for("https://test-admin.x.com", "/orders"),
            Some(SiteMatch {
                path_scope: "/".into(),
                wildcard: false
            })
        );
        assert_eq!(
            c.site_for("https://a.preview.x.com", "/admin/x"),
            Some(SiteMatch {
                path_scope: "/admin".into(),
                wildcard: true
            })
        );
        assert_eq!(
            c.site_for("https://a.preview.x.com", "/administrator"),
            None
        );
        assert_eq!(c.site_for("https://a.b.preview.x.com", "/admin"), None);
        assert!(c.site_with_scope("http://localhost:5173", "/").is_some());
        assert!(c
            .site_with_scope("http://localhost:5173", "/admin")
            .is_none());
        // A root-level config makes the worktree itself the app root.
        write(
            &top,
            ".chrome-use/inspect.json",
            r#"{"version":1,"sites":[{"origins":["http://localhost:3000"]}]}"#,
        );
        let r = load(&top, ".chrome-use/inspect.json").unwrap();
        assert_eq!(r.app_root, top);
        assert_eq!(r.source_roots, std::slice::from_ref(&top));
    }

    #[test]
    fn configs_outside_the_schema_are_rejected_with_a_reason() {
        let (_t, top) = worktree();
        let reject = |body: &str| {
            write(&top, REL, body);
            load(&top, REL).unwrap_err()
        };
        assert!(reject(r#"{"version":2,"sites":[]}"#).contains("unsupported version 2"));
        assert!(reject(r#"{"sites":[]}"#).contains("version"));
        assert!(reject(
            r#"{"version":1,"sites":[{"origins":["http://localhost:1"]}],"extra":true}"#
        )
        .contains("unknown field"));
        assert!(reject(
            r#"{"version":1,"sites":[{"origins":["http://localhost:1"],"root":"/etc"}]}"#
        )
        .contains("unknown field"));
        let many: Vec<String> = (0..33)
            .map(|i| format!(r#"{{"origins":["http://localhost:{}"]}}"#, 3000 + i))
            .collect();
        assert!(
            reject(&format!(r#"{{"version":1,"sites":[{}]}}"#, many.join(","))).contains("sites")
        );
        assert!(
            reject(r#"{"version":1,"sites":[{"origins":["https://*.com"]}]}"#)
                .contains("two labels")
        );
        assert!(reject(
            r#"{"version":1,"sites":[{"origins":["http://localhost:1"],"pathScope":"/a/"}]}"#
        )
        .contains("pathScope"));
        assert!(reject("not json").contains("invalid JSON"));
        for escape in ["../../../outside", "/etc", "../../packages/missing"] {
            let body = format!(
                r#"{{"version":1,"sourceRoots":["{escape}"],"sites":[{{"origins":["http://localhost:1"]}}]}}"#
            );
            assert!(reject(&body).contains("sourceRoots"), "{escape} accepted");
        }
        let big = format!(
            r#"{{"version":1,"sites":[{{"origins":["http://localhost:1"]}}],"pad":"{}"}}"#,
            "x".repeat(70 * 1024)
        );
        assert!(reject(&big).contains("KiB"));
        assert!(load(&top, "../x/.chrome-use/inspect.json").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_configs_and_directories_are_not_read() {
        let (_t, top) = worktree();
        let outside = top.parent().unwrap().join("outside");
        fs::create_dir_all(outside.join(".chrome-use")).unwrap();
        let body = r#"{"version":1,"sites":[{"origins":["http://localhost:1"]}]}"#;
        fs::write(outside.join(".chrome-use/inspect.json"), body).unwrap();
        std::os::unix::fs::symlink(outside.join(".chrome-use/inspect.json"), top.join(REL))
            .unwrap();
        assert!(load(&top, REL).unwrap_err().contains("symlink"));
        std::os::unix::fs::symlink(&outside, top.join("apps/linked")).unwrap();
        assert!(load(&top, "apps/linked/.chrome-use/inspect.json")
            .unwrap_err()
            .contains("symlink"));
        // A source root that is a symlink out of the worktree fails too.
        std::os::unix::fs::symlink(&outside, top.join("packages/escape")).unwrap();
        fs::remove_file(top.join(REL)).unwrap();
        write(
            &top,
            REL,
            r#"{"version":1,"sourceRoots":["../../packages/escape"],"sites":[{"origins":["http://localhost:1"]}]}"#,
        );
        assert!(load(&top, REL)
            .unwrap_err()
            .contains("outside the worktree"));
    }
}
