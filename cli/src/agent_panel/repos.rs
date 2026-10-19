//! Which local checkout a code-hosting page belongs to.
//!
//! A page such as `github.com/owner/repo/pull/5` names a repository; a
//! recent project whose git remote (`origin`, `upstream`, any other) points
//! at the same host and repository path is that page's project. Matching is
//! by the remotes themselves, so GitLab subgroups and self-hosted Gitea,
//! Gogs or GitLab servers need no list of hosts. Remotes are read from
//! `.git/config` directly and cached by the file's modification time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Hosts known to serve repositories at `/owner/repo`.
const CODE_HOSTS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "gitee.com",
    "bitbucket.org",
    "codeberg.org",
];

/// First path segments of known hosts that are not an owner.
const NOT_OWNERS: &[&str] = &[
    "about",
    "account",
    "apps",
    "codespaces",
    "collections",
    "dashboard",
    "enterprise",
    "explore",
    "features",
    "groups",
    "help",
    "issues",
    "login",
    "logout",
    "marketplace",
    "new",
    "notifications",
    "orgs",
    "organizations",
    "pricing",
    "pulls",
    "search",
    "settings",
    "signup",
    "sponsors",
    "topics",
    "trending",
    "users",
];

/// Third path segments of a Gitea or Gogs repository page.
const REPO_TABS: &[&str] = &[
    "actions",
    "activity",
    "blob",
    "branches",
    "commit",
    "commits",
    "compare",
    "issues",
    "milestones",
    "packages",
    "projects",
    "pull",
    "pulls",
    "raw",
    "releases",
    "settings",
    "src",
    "tags",
    "tree",
    "wiki",
];

/// A remote as host plus repository path, both lowercase, the path without
/// `.git` or surrounding slashes: `github.com` + `owner/repo`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Remote {
    pub host: String,
    pub path: String,
}

/// One `[remote "name"]` url of a checkout.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NamedRemote {
    pub name: String,
    pub remote: Remote,
}

fn host_of(raw: &str) -> Option<String> {
    let host = raw.rsplit('@').next()?;
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => host.split(':').next()?,
    };
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    // SSH over port 443 is served from another name.
    let host = match host {
        "ssh.github.com" => "github.com",
        "altssh.gitlab.com" => "gitlab.com",
        "altssh.bitbucket.org" => "bitbucket.org",
        other => other,
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// A git remote URL in any form git accepts for a server: `https://`,
/// `http://`, `ssh://`, `git://`, and scp-like `user@host:owner/repo.git`.
/// Local paths and `file://` URLs are not remotes of a web page.
#[cfg(test)]
fn parse_remote(url: &str) -> Option<Remote> {
    parse_remote_via(url, |_| None)
}

/// `parse_remote`, with an ssh host alias (`git@github-work:o/r`) turned
/// into its `HostName` by `alias`.
fn parse_remote_via(url: &str, alias: impl Fn(&str) -> Option<String>) -> Option<Remote> {
    let url = url.trim();
    let mut ssh = true;
    let (authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !["https", "http", "ssh", "git", "git+ssh", "ssh+git"].contains(&scheme.as_str()) {
            return None;
        }
        ssh = scheme.contains("ssh");
        rest.split_once('/').unwrap_or((rest, ""))
    } else {
        // git reads `host:path` as scp-like only when no slash precedes the
        // colon; a one-letter host is a Windows drive.
        let (host, path) = url.split_once(':')?;
        if host.contains('/') || host.rsplit('@').next()?.len() < 2 {
            return None;
        }
        (host, path)
    };
    let host = host_of(authority)?;
    let host = match ssh.then(|| alias(&host)).flatten() {
        Some(real) => host_of(&real)?,
        None => host,
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    let path = path.strip_prefix("~/").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2
        || segments
            .iter()
            .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return None;
    }
    Some(Remote {
        host,
        path: path.to_ascii_lowercase(),
    })
}

/// The page's path segments up to GitLab's `/-/` separator.
fn repo_segments(page_path: &str) -> Vec<&str> {
    page_path
        .split('/')
        .filter(|s| !s.is_empty())
        .take_while(|s| *s != "-")
        .collect()
}

fn scope_of(segments: &[&str]) -> String {
    format!("/{}", segments.join("/"))
}

/// Whether the host is a known code host.
pub(super) fn known_host(host: &str) -> bool {
    host_of(host).is_some_and(|h| CODE_HOSTS.contains(&h.as_str()))
}

/// The repository a page names by its path shape alone, as a path scope in
/// the page's own casing: `/owner/repo`, GitLab's `/group/sub/repo` before
/// `/-/`, a Gitea or Gogs `/owner/repo/src/…`. None when the shape says
/// nothing (an unknown host's `/a/b` may be anything).
pub(super) fn shape_scope(host: &str, page_path: &str) -> Option<String> {
    let all: Vec<&str> = page_path.split('/').filter(|s| !s.is_empty()).collect();
    let before = repo_segments(page_path);
    if before.len() < all.len() && before.len() >= 2 {
        return Some(scope_of(&before));
    }
    if all.len() < 2 {
        return None;
    }
    let host = host_of(host)?;
    if CODE_HOSTS.contains(&host.as_str()) {
        if NOT_OWNERS.contains(&all[0].to_ascii_lowercase().as_str()) {
            return None;
        }
        // A GitLab project's own pages all carry `/-/`; without it the whole
        // path is the project (or a group: the remember bar only proposes it).
        return Some(scope_of(if host == "gitlab.com" {
            &all
        } else {
            &all[..2]
        }));
    }
    let tab = all.get(2)?.to_ascii_lowercase();
    REPO_TABS
        .contains(&tab.as_str())
        .then(|| scope_of(&all[..2]))
}

/// A recent project whose remote names the page's repository.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Match {
    pub root: PathBuf,
    /// The remote's name in the checkout.
    pub remote: String,
    /// The repository as a path scope in the page's casing.
    pub scope: String,
}

/// The candidates (most recent first, with their remotes) whose remote is
/// the page's repository. The longest repository path wins (a GitLab
/// subgroup project over its group); among equals a checkout's `origin`
/// goes before its other remotes, then the more recent checkout.
pub(super) fn matches(
    host: &str,
    page_path: &str,
    candidates: &[(PathBuf, Vec<NamedRemote>)],
) -> Vec<Match> {
    let Some(host) = host_of(host) else {
        return Vec::new();
    };
    let segments = repo_segments(page_path);
    let lower: Vec<String> = segments.iter().map(|s| s.to_ascii_lowercase()).collect();
    let mut found: Vec<(usize, bool, usize, Match)> = Vec::new();
    for (order, (root, remotes)) in candidates.iter().enumerate() {
        let best = remotes
            .iter()
            .filter(|r| r.remote.host == host)
            .filter_map(|r| {
                let parts: Vec<&str> = r.remote.path.split('/').collect();
                (parts.len() <= lower.len() && parts.iter().zip(&lower).all(|(a, b)| a == b))
                    .then_some((parts.len(), r.name == "origin", r))
            })
            .max_by_key(|(len, origin, _)| (*len, *origin));
        if let Some((len, origin, r)) = best {
            found.push((
                len,
                origin,
                order,
                Match {
                    root: root.clone(),
                    remote: r.name.clone(),
                    scope: scope_of(&segments[..len]),
                },
            ));
        }
    }
    let Some(longest) = found.iter().map(|f| f.0).max() else {
        return Vec::new();
    };
    found.retain(|f| f.0 == longest);
    found.sort_by_key(|(_, origin, order, _)| (!*origin, *order));
    found.into_iter().map(|f| f.3).collect()
}

/// The `config` file of the repository holding `dir`, found by walking up.
/// A worktree's `.git` is a file naming its git directory, whose
/// `commondir` leads to the shared config.
fn config_of(dir: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir();
    for at in dir.ancestors().take(12) {
        if home.as_deref() == Some(at) {
            return None;
        }
        let dot = at.join(".git");
        let Ok(meta) = std::fs::metadata(&dot) else {
            continue;
        };
        if meta.is_dir() {
            return Some(dot.join("config"));
        }
        let text = std::fs::read_to_string(&dot).ok()?;
        let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        let gitdir = at.join(gitdir);
        let common = match std::fs::read_to_string(gitdir.join("commondir")) {
            Ok(common) => gitdir.join(common.trim()),
            Err(_) => gitdir,
        };
        return Some(common.join("config"));
    }
    None
}

/// Every remote url in a git config file's text.
pub(super) fn config_remotes(text: &str) -> Vec<NamedRemote> {
    let mut remotes = Vec::new();
    let mut section: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[') {
            let header = header.split(']').next().unwrap_or("").trim();
            section = header
                .strip_prefix("remote")
                .and_then(|rest| {
                    let rest = rest.trim();
                    rest.strip_prefix('"')
                        .and_then(|r| r.strip_suffix('"'))
                        .or_else(|| rest.strip_prefix('.'))
                })
                .map(str::to_string);
            continue;
        }
        let Some(name) = &section else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("url") {
            continue;
        }
        let value = value.trim();
        let value = match value.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next().unwrap_or(""),
            None => value.split([';', '#']).next().unwrap_or("").trim(),
        };
        if let Some(remote) = parse_remote_via(value, ssh_host_name) {
            remotes.push(NamedRemote {
                name: name.clone(),
                remote,
            });
        }
    }
    remotes
}

/// `Host alias` → `HostName` pairs of an ssh config: exact aliases only
/// (no patterns), the first `HostName` of a block, in file order.
pub(super) fn ssh_host_names(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    let mut named = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k,
                v.trim_start_matches(|c: char| c.is_whitespace() || c == '=')
                    .trim(),
            ),
            None => continue,
        };
        if key.eq_ignore_ascii_case("host") || key.eq_ignore_ascii_case("match") {
            named = false;
            aliases = if key.eq_ignore_ascii_case("host") {
                value
                    .split_whitespace()
                    .filter(|a| !a.contains(['*', '?', '!']))
                    .map(str::to_ascii_lowercase)
                    .collect()
            } else {
                Vec::new()
            };
        } else if key.eq_ignore_ascii_case("hostname") && !named {
            named = true;
            let real = value.trim_matches('"').to_ascii_lowercase();
            pairs.extend(aliases.iter().map(|a| (a.clone(), real.clone())));
        }
    }
    pairs
}

type HostNames = Vec<(String, String)>;

/// The `HostName` `~/.ssh/config` gives an ssh host alias, read once per
/// change of the file (`Include`d files are not followed).
fn ssh_host_name(alias: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<Option<(SystemTime, HostNames)>>> = OnceLock::new();
    let config = dirs::home_dir()?.join(".ssh").join("config");
    let modified = std::fs::metadata(&config).and_then(|m| m.modified()).ok()?;
    let cache = CACHE.get_or_init(Default::default);
    let mut cached = cache.lock().ok()?;
    if cached.as_ref().is_none_or(|(at, _)| *at != modified) {
        let text = std::fs::read_to_string(&config).unwrap_or_default();
        *cached = Some((modified, ssh_host_names(&text)));
    }
    let alias = alias.to_ascii_lowercase();
    cached
        .as_ref()?
        .1
        .iter()
        .find(|(a, _)| *a == alias)
        .map(|(_, real)| real.clone())
}

type Cache = HashMap<PathBuf, (SystemTime, Vec<NamedRemote>)>;

/// The remotes of the repository holding `dir`; none outside a repository.
pub(super) fn remotes_of(dir: &Path) -> Vec<NamedRemote> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let Some(config) = config_of(dir) else {
        return Vec::new();
    };
    let Ok(modified) = std::fs::metadata(&config).and_then(|m| m.modified()) else {
        return Vec::new();
    };
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, remotes)) = cache.lock().ok().and_then(|c| c.get(&config).cloned()) {
        if at == modified {
            return remotes;
        }
    }
    let remotes = std::fs::read_to_string(&config)
        .map(|text| config_remotes(&text))
        .unwrap_or_default();
    if let Ok(mut c) = cache.lock() {
        c.insert(config, (modified, remotes.clone()));
    }
    remotes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(host: &str, path: &str) -> Option<Remote> {
        Some(Remote {
            host: host.into(),
            path: path.into(),
        })
    }

    #[test]
    fn remote_urls_in_every_form() {
        for url in [
            "https://github.com/leeguooooo/image-use.git",
            "https://github.com/leeguooooo/image-use",
            "https://github.com/LeeGuooooo/Image-Use/",
            "http://user:token@github.com/leeguooooo/image-use.git",
            "https://www.github.com/leeguooooo/image-use",
            "ssh://git@github.com/leeguooooo/image-use.git",
            "ssh://git@ssh.github.com:443/leeguooooo/image-use.git",
            "git@github.com:leeguooooo/image-use.git",
            "github.com:leeguooooo/image-use",
            "git://github.com/leeguooooo/image-use",
            "git+ssh://git@github.com/leeguooooo/image-use.git/",
        ] {
            assert_eq!(
                parse_remote(url),
                remote("github.com", "leeguooooo/image-use"),
                "{url}"
            );
        }
        assert_eq!(
            parse_remote("git@gitlab.com:group/sub/team/app.git"),
            remote("gitlab.com", "group/sub/team/app")
        );
        assert_eq!(
            parse_remote("ssh://git@sg-git-src.pwtk.cc:2222/Team/Api.git"),
            remote("sg-git-src.pwtk.cc", "team/api")
        );
        assert_eq!(parse_remote("https://[::1]:3000/o/r"), remote("::1", "o/r"));
        for bad in [
            "/Users/me/repo",
            "../repo",
            "file:///Users/me/repo.git",
            "C:\\work\\repo",
            "C:/work/repo",
            "https://github.com/onlyowner",
            "https://github.com/",
            "ftp://host/o/r",
            "",
        ] {
            assert_eq!(parse_remote(bad), None, "{bad}");
        }
    }

    #[test]
    fn ssh_host_aliases_resolve_to_their_host_name() {
        let config = "# work account\nHost github-work gw\n  HostName github.com\n  IdentityFile ~/.ssh/work\nHost *.internal\n  HostName ignored.example\nHost sg-git\n  HostName=sg-git-src.pwtk.cc\n  Port 2222\nMatch host foo\n  HostName bar.example\n";
        let pairs = ssh_host_names(config);
        assert_eq!(
            pairs,
            [
                ("github-work".to_string(), "github.com".to_string()),
                ("gw".into(), "github.com".into()),
                ("sg-git".into(), "sg-git-src.pwtk.cc".into()),
            ]
        );
        let alias = |host: &str| {
            pairs
                .iter()
                .find(|(a, _)| a == host)
                .map(|(_, r)| r.clone())
        };
        assert_eq!(
            parse_remote_via("git@github-work:leeguooooo/image-use.git", alias),
            remote("github.com", "leeguooooo/image-use")
        );
        assert_eq!(
            parse_remote_via("ssh://git@sg-git/team/api.git", alias),
            remote("sg-git-src.pwtk.cc", "team/api")
        );
        // Only ssh remotes go through ssh's config.
        assert_eq!(
            parse_remote_via("https://gw/o/r", alias),
            remote("gw", "o/r")
        );
    }

    #[test]
    fn config_remotes_read_every_remote_section() {
        let text = r#"
[core]
	url = https://github.com/not/a-remote
[remote "origin"]
	url = git@github.com:leeguooooo/image-use.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[remote "upstream"]
	URL = "https://gitlab.com/group/sub/app.git" ; mirror
[remote.old]
	url = /local/path/only
[branch "main"]
	remote = origin
"#;
        assert_eq!(
            config_remotes(text),
            vec![
                NamedRemote {
                    name: "origin".into(),
                    remote: remote("github.com", "leeguooooo/image-use").unwrap()
                },
                NamedRemote {
                    name: "upstream".into(),
                    remote: remote("gitlab.com", "group/sub/app").unwrap()
                },
            ]
        );
    }

    #[test]
    fn page_shapes_name_a_repository() {
        let s = |host: &str, path: &str| shape_scope(host, path);
        assert_eq!(
            s("github.com", "/leeguooooo/image-use/pull/55").as_deref(),
            Some("/leeguooooo/image-use")
        );
        assert_eq!(s("github.com", "/leeguooooo").as_deref(), None);
        assert_eq!(s("github.com", "/settings/profile"), None);
        assert_eq!(s("github.com", "/"), None);
        assert_eq!(
            s("gitlab.com", "/group/sub/app/-/merge_requests/3").as_deref(),
            Some("/group/sub/app")
        );
        assert_eq!(
            s("gitlab.com", "/group/sub/app").as_deref(),
            Some("/group/sub/app")
        );
        assert_eq!(s("gitlab.com", "/explore/projects"), None);
        assert_eq!(
            s("bitbucket.org", "/team/repo/src/main/").as_deref(),
            Some("/team/repo")
        );
        // Self-hosted: GitLab's separator or a Gitea/Gogs tab.
        assert_eq!(
            s("git.example.com:8443", "/ops/infra/deploy/-/tree/main").as_deref(),
            Some("/ops/infra/deploy")
        );
        assert_eq!(
            s("sg-git-src.pwtk.cc", "/Team/Api/src/branch/main").as_deref(),
            Some("/Team/Api")
        );
        assert_eq!(s("sg-git-src.pwtk.cc", "/Team/Api/whatever"), None);
        assert_eq!(s("docs.example.com", "/guide/install"), None);
        assert!(known_host("GitHub.com") && known_host("www.gitee.com"));
        assert!(!known_host("sg-git-src.pwtk.cc"));
    }

    #[test]
    fn local_remotes_pick_the_page_repository() {
        let named = |name: &str, url: &str| NamedRemote {
            name: name.into(),
            remote: parse_remote(url).unwrap(),
        };
        let candidates = vec![
            (
                PathBuf::from("/w/chatgpt-imagegen"),
                vec![named(
                    "origin",
                    "git@github.com:leeguooooo/chatgpt-imagegen.git",
                )],
            ),
            (
                PathBuf::from("/w/image-use-fork"),
                vec![
                    named("origin", "git@github.com:someone/image-use.git"),
                    named("upstream", "https://github.com/leeguooooo/image-use"),
                ],
            ),
            (
                PathBuf::from("/w/image-use"),
                vec![named(
                    "origin",
                    "https://github.com/LeeGuooooo/Image-Use.git",
                )],
            ),
            (
                PathBuf::from("/w/group"),
                vec![named("origin", "git@gitlab.com:group/sub.git")],
            ),
            (
                PathBuf::from("/w/app"),
                vec![named("origin", "ssh://git@gitlab.com/group/sub/app.git")],
            ),
            (
                PathBuf::from("/w/api"),
                vec![named("origin", "git@sg-git-src.pwtk.cc:team/api.git")],
            ),
        ];
        let roots = |host: &str, path: &str| {
            matches(host, path, &candidates)
                .into_iter()
                .map(|m| (m.root.to_string_lossy().to_string(), m.remote, m.scope))
                .collect::<Vec<_>>()
        };
        // Case-insensitive; the checkout whose origin is the repository
        // goes before a fork tracking it as upstream.
        assert_eq!(
            roots("github.com", "/leeguooooo/image-use/pull/55"),
            [
                (
                    "/w/image-use".into(),
                    "origin".into(),
                    "/leeguooooo/image-use".into()
                ),
                (
                    "/w/image-use-fork".into(),
                    "upstream".into(),
                    "/leeguooooo/image-use".into()
                ),
            ]
        );
        assert!(roots("github.com", "/leeguooooo/image-user").is_empty());
        assert!(roots("github.com", "/leeguooooo").is_empty());
        assert!(roots("gitee.com", "/leeguooooo/image-use").is_empty());
        // A subgroup's project beats its group.
        assert_eq!(
            roots("gitlab.com", "/group/sub/app/-/issues/1"),
            [("/w/app".into(), "origin".into(), "/group/sub/app".into())]
        );
        assert_eq!(roots("gitlab.com", "/group/sub/-/issues")[0].0, "/w/group");
        // Self-hosted, any port, matched by the remote alone.
        assert_eq!(
            roots("sg-git-src.pwtk.cc", "/Team/Api/pulls/7"),
            [("/w/api".into(), "origin".into(), "/Team/Api".into())]
        );
    }

    #[test]
    fn remotes_are_read_from_checkouts_worktrees_and_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        std::fs::write(
            main.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:o/r.git\n",
        )
        .unwrap();
        std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(wt.join("src/deep")).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        let expected = remote("github.com", "o/r");
        assert_eq!(remotes_of(&main)[0].remote, expected.clone().unwrap());
        assert_eq!(remotes_of(&wt)[0].remote, expected.clone().unwrap());
        assert_eq!(
            remotes_of(&wt.join("src/deep"))[0].remote,
            expected.unwrap()
        );
        // A changed config is read again.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            main.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://gitlab.com/g/p\n",
        )
        .unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(main.join(".git/config"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(remotes_of(&wt)[0].remote.host, "gitlab.com");
        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert!(remotes_of(&plain).is_empty());
    }
}
