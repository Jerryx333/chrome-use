//! Sites that hold many projects (App Store Connect, Vercel, Netlify,
//! Cloudflare, Firebase and Google Cloud, Sentry): which project a page is
//! about, read from its URL by a table of rules, and which recent project
//! folder names that same project in a small file of its own
//! (`.vercel/project.json`, `wrangler.toml`, `.firebaserc`, …). Code hosts
//! are matched by git remote instead (`repos`).
//!
//! A rule is data: add a row to `RULES` to teach the panel another host.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// What kind of project a page names; decides which local files can name it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    AppStoreApp,
    Vercel,
    Netlify,
    CloudflareWorker,
    CloudflarePages,
    /// A Firebase or Google Cloud project id (one namespace).
    GoogleCloud,
    Sentry,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::AppStoreApp => "appStoreApp",
            Self::Vercel => "vercel",
            Self::Netlify => "netlify",
            Self::CloudflareWorker => "cloudflareWorker",
            Self::CloudflarePages => "cloudflarePages",
            Self::GoogleCloud => "googleCloud",
            Self::Sentry => "sentry",
        }
    }
}

/// One way a host's URLs name a project.
struct Rule {
    /// Exact host, or `.suffix` for every subdomain of it.
    host: &'static str,
    /// Path segments: a literal, `*` for any one segment, `{}` for a segment
    /// of the project key. The page path must start with all of them.
    path: &'static [&'static str],
    /// First segments that are site pages rather than a project.
    reserved: &'static [&'static str],
    /// Query parameter holding the key when the path does not.
    query: Option<&'static str>,
    kind: Kind,
}

const VERCEL_PAGES: &[&str] = &[
    "account",
    "ai",
    "api",
    "blog",
    "changelog",
    "contact",
    "customers",
    "dashboard",
    "docs",
    "enterprise",
    "frameworks",
    "guides",
    "home",
    "integrations",
    "legal",
    "login",
    "marketplace",
    "new",
    "pricing",
    "products",
    "signup",
    "solutions",
    "support",
    "teams",
    "templates",
];

const SENTRY_PAGES: &[&str] = &[
    "accounts",
    "api",
    "auth",
    "issues",
    "join-request",
    "onboarding",
    "organizations",
    "out",
    "pricing",
    "projects",
    "settings",
    "share",
    "welcome",
];

/// First match wins, so a host's narrower rules come first.
const RULES: &[Rule] = &[
    // App Store Connect: /apps/<appId>/appstore, …/testflight/ios, …
    Rule {
        host: "appstoreconnect.apple.com",
        path: &["apps", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::AppStoreApp,
    },
    Rule {
        host: "appstoreconnect.apple.com",
        path: &["teams", "*", "apps", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::AppStoreApp,
    },
    Rule {
        host: "vercel.com",
        path: &["{}", "{}"],
        reserved: VERCEL_PAGES,
        query: None,
        kind: Kind::Vercel,
    },
    Rule {
        host: "app.netlify.com",
        path: &["sites", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::Netlify,
    },
    Rule {
        host: "app.netlify.com",
        path: &["projects", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::Netlify,
    },
    Rule {
        host: "dash.cloudflare.com",
        path: &["*", "workers", "services", "view", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::CloudflareWorker,
    },
    Rule {
        host: "dash.cloudflare.com",
        path: &["*", "pages", "view", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::CloudflarePages,
    },
    Rule {
        host: "console.firebase.google.com",
        path: &["project", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::GoogleCloud,
    },
    Rule {
        host: "console.firebase.google.com",
        path: &["u", "*", "project", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::GoogleCloud,
    },
    Rule {
        host: "console.cloud.google.com",
        path: &[],
        reserved: &[],
        query: Some("project"),
        kind: Kind::GoogleCloud,
    },
    Rule {
        host: "sentry.io",
        path: &["organizations", "{}", "projects", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::Sentry,
    },
    Rule {
        host: "sentry.io",
        path: &["settings", "{}", "projects", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::Sentry,
    },
    Rule {
        host: "sentry.io",
        path: &["organizations", "{}"],
        reserved: &[],
        query: Some("project"),
        kind: Kind::Sentry,
    },
    // The older /<org>/<project>/ pages.
    Rule {
        host: "sentry.io",
        path: &["{}", "{}"],
        reserved: SENTRY_PAGES,
        query: None,
        kind: Kind::Sentry,
    },
    // <org>.sentry.io: the organization is the subdomain.
    Rule {
        host: ".sentry.io",
        path: &["projects", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::Sentry,
    },
    Rule {
        host: ".sentry.io",
        path: &["settings", "projects", "{}"],
        reserved: &[],
        query: None,
        kind: Kind::Sentry,
    },
    Rule {
        host: ".sentry.io",
        path: &[],
        reserved: &[],
        query: Some("project"),
        kind: Kind::Sentry,
    },
];

/// The project a page is about.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageProject {
    pub kind: Kind,
    /// The key segments joined with `/` (`team/project`), or the query value.
    pub key: String,
    /// What a local file names: the last key segment, or the query value.
    pub local: String,
    /// The page path through the rule's last segment, in the page's casing;
    /// None when the key is in the query (a binding cannot hold it).
    pub scope: Option<String>,
}

fn host_matches(rule: &str, host: &str) -> bool {
    match rule.strip_prefix('.') {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|sub| sub.ends_with('.') && sub.len() > 1),
        None => host == rule,
    }
}

/// Whether some rule knows `host`: a site of many projects, never bound
/// whole by default.
pub(super) fn known_host(host: &str) -> bool {
    let host = bare_host(host);
    RULES.iter().any(|r| host_matches(r.host, &host))
}

fn bare_host(host: &str) -> String {
    host.split(':').next().unwrap_or("").to_ascii_lowercase()
}

/// The project `host` + `page_path` (+ `query`, as `location.search`) is
/// about, by the first rule that applies.
pub(super) fn page_project(host: &str, page_path: &str, query: &str) -> Option<PageProject> {
    let host = bare_host(host);
    let segments: Vec<&str> = page_path.split('/').filter(|s| !s.is_empty()).collect();
    let pairs: Vec<(String, String)> =
        url::form_urlencoded::parse(query.trim_start_matches('?').as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
    RULES.iter().find_map(|rule| {
        if !host_matches(rule.host, &host) || segments.len() < rule.path.len() {
            return None;
        }
        let first = segments.first().map(|s| s.to_ascii_lowercase());
        if first.is_some_and(|f| rule.reserved.contains(&f.as_str())) {
            return None;
        }
        let mut key = Vec::new();
        for (pattern, segment) in rule.path.iter().zip(&segments) {
            match *pattern {
                "*" => {}
                "{}" => key.push(*segment),
                literal if literal.eq_ignore_ascii_case(segment) => {}
                _ => return None,
            }
        }
        match rule.query {
            Some(name) => {
                let value = pairs
                    .iter()
                    .find(|(k, v)| k == name && !v.is_empty())?
                    .1
                    .clone();
                key.push(value.as_str());
                Some(PageProject {
                    kind: rule.kind,
                    key: key.join("/"),
                    local: value.clone(),
                    scope: None,
                })
            }
            None => Some(PageProject {
                kind: rule.kind,
                key: key.join("/"),
                local: key.last()?.to_string(),
                scope: Some(format!("/{}", segments[..rule.path.len()].join("/"))),
            }),
        }
    })
}

/// How a file names a project.
enum Parse {
    /// The first `"field": "value"` of a JSON (or JSONC) file.
    Json(&'static str),
    /// A top-level `name = "…"` of wrangler.toml.
    TomlName,
    /// Every project id of `.firebaserc`.
    Firebaserc,
    /// `key=value` lines of an ini or properties file.
    Ini(&'static str),
    /// App ids on fastlane lines.
    AppIds,
}

/// Files in a project's root that name its hosted projects, and how.
const FILES: &[(&str, Kind, Parse)] = &[
    (
        ".vercel/project.json",
        Kind::Vercel,
        Parse::Json("projectName"),
    ),
    ("vercel.json", Kind::Vercel, Parse::Json("name")),
    ("wrangler.toml", Kind::CloudflareWorker, Parse::TomlName),
    ("wrangler.json", Kind::CloudflareWorker, Parse::Json("name")),
    (
        "wrangler.jsonc",
        Kind::CloudflareWorker,
        Parse::Json("name"),
    ),
    (".firebaserc", Kind::GoogleCloud, Parse::Firebaserc),
    (".sentryclirc", Kind::Sentry, Parse::Ini("project")),
    (
        "sentry.properties",
        Kind::Sentry,
        Parse::Ini("defaults.project"),
    ),
    ("fastlane/Appfile", Kind::AppStoreApp, Parse::AppIds),
    ("fastlane/Fastfile", Kind::AppStoreApp, Parse::AppIds),
    ("fastlane/Deliverfile", Kind::AppStoreApp, Parse::AppIds),
];

impl Parse {
    fn values(&self, text: &str) -> Vec<String> {
        match self {
            Self::Json(field) => string_field(text, field),
            Self::TomlName => toml_name(text),
            Self::Firebaserc => firebaserc(text),
            Self::Ini(key) => ini(text, key),
            Self::AppIds => app_ids(text),
        }
    }
}

fn string_field(text: &str, field: &str) -> Vec<String> {
    let needle = format!("\"{field}\"");
    let Some(at) = text.find(&needle) else {
        return Vec::new();
    };
    let rest = text[at + needle.len()..].trim_start();
    let Some(rest) = rest.strip_prefix(':') else {
        return Vec::new();
    };
    quoted(rest.trim_start()).into_iter().collect()
}

fn quoted(text: &str) -> Option<String> {
    let quote = text.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let body = &text[1..];
    let end = body.find(quote)?;
    Some(body[..end].to_string()).filter(|v| !v.is_empty())
}

/// A top-level `name = "…"` of wrangler.toml (before any table).
fn toml_name(text: &str) -> Vec<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "name" {
                return quoted(value.trim()).into_iter().collect();
            }
        }
    }
    Vec::new()
}

/// Every project id of `.firebaserc` (`{"projects": {"default": "id"}}`).
fn firebaserc(text: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v["projects"].as_object().map(|projects| {
                projects
                    .values()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn ini(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// App Store Connect app ids (6 to 12 digits) quoted on fastlane lines that
/// name one: `apple_id("1234567890")`, `app_id: "1234567890"`.
fn app_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !["apple_id", "app_id", "apple-id"]
            .iter()
            .any(|k| lower.contains(k))
        {
            continue;
        }
        for part in line.split(['"', '\'']).skip(1).step_by(2) {
            if (6..=12).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()) {
                ids.push(part.to_string());
            }
        }
    }
    ids
}

/// Read files above this size are not project metadata.
const MAX_FILE: u64 = 64 * 1024;

type Cache = HashMap<PathBuf, (SystemTime, Vec<String>)>;

/// What `dir`'s own files name, per kind; each file read once per change.
pub(super) fn local_keys(dir: &Path) -> Vec<(Kind, String)> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut keys = Vec::new();
    for (name, kind, parse) in FILES {
        let path = dir.join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_FILE {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let cached = cache
            .lock()
            .ok()
            .and_then(|c| c.get(&path).cloned())
            .filter(|(at, _)| *at == modified);
        let values = match cached {
            Some((_, values)) => values,
            None => {
                let values = std::fs::read_to_string(&path)
                    .map(|text| parse.values(&text))
                    .unwrap_or_default();
                if let Ok(mut c) = cache.lock() {
                    c.insert(path, (modified, values.clone()));
                }
                values
            }
        };
        keys.extend(values.into_iter().map(|v| (*kind, v)));
    }
    keys
}

/// Whether a local file of kind `file` can name a page project of `page`.
fn same_namespace(page: Kind, file: Kind) -> bool {
    page == file
        || matches!(
            (page, file),
            (Kind::CloudflarePages, Kind::CloudflareWorker)
        )
}

/// The candidates (most recent first) whose files name the page's project.
pub(super) fn matches(
    project: &PageProject,
    candidates: &[(PathBuf, Vec<(Kind, String)>)],
) -> Vec<PathBuf> {
    candidates
        .iter()
        .filter(|(_, keys)| {
            keys.iter().any(|(kind, value)| {
                same_namespace(project.kind, *kind) && value.eq_ignore_ascii_case(&project.local)
            })
        })
        .map(|(root, _)| root.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(url: &str) -> Option<(Kind, String, String, Option<String>)> {
        let u = url::Url::parse(url).unwrap();
        let host = match u.port() {
            Some(port) => format!("{}:{port}", u.host_str().unwrap()),
            None => u.host_str().unwrap().to_string(),
        };
        page_project(
            &host,
            u.path(),
            u.query().map(|q| format!("?{q}")).as_deref().unwrap_or(""),
        )
        .map(|p| (p.kind, p.key, p.local, p.scope))
    }

    fn scoped(
        kind: Kind,
        key: &str,
        local: &str,
        scope: &str,
    ) -> Option<(Kind, String, String, Option<String>)> {
        Some((kind, key.into(), local.into(), Some(scope.into())))
    }

    #[test]
    fn page_urls_name_their_project() {
        use Kind::*;
        for (url, want) in [
            (
                "https://appstoreconnect.apple.com/apps/1234567890/appstore/ios/version/inflight",
                scoped(AppStoreApp, "1234567890", "1234567890", "/apps/1234567890"),
            ),
            (
                "https://appstoreconnect.apple.com/apps/1234567890/testflight/ios/9f1c2d3e-0000-4b4b-8c8c-123456789abc",
                scoped(AppStoreApp, "1234567890", "1234567890", "/apps/1234567890"),
            ),
            (
                "https://appstoreconnect.apple.com/teams/69a6de8f-1234-47e3-e053-5b8c7c11a4d1/apps/6449012345/testflight/groups",
                scoped(AppStoreApp, "6449012345", "6449012345", "/teams/69a6de8f-1234-47e3-e053-5b8c7c11a4d1/apps/6449012345"),
            ),
            ("https://appstoreconnect.apple.com/apps", None),
            ("https://appstoreconnect.apple.com/access/users", None),
            (
                "https://vercel.com/acme-team/marketing-site/deployments",
                scoped(Vercel, "acme-team/marketing-site", "marketing-site", "/acme-team/marketing-site"),
            ),
            ("https://vercel.com/acme-team", None),
            ("https://vercel.com/docs/projects/overview", None),
            ("https://vercel.com/new/clone", None),
            (
                "https://app.netlify.com/sites/brave-curie-1a2b3c/deploys",
                scoped(Netlify, "brave-curie-1a2b3c", "brave-curie-1a2b3c", "/sites/brave-curie-1a2b3c"),
            ),
            (
                "https://app.netlify.com/projects/docs-site/overview",
                scoped(Netlify, "docs-site", "docs-site", "/projects/docs-site"),
            ),
            ("https://app.netlify.com/teams/acme/sites", None),
            (
                "https://dash.cloudflare.com/0123456789abcdef0123456789abcdef/workers/services/view/api-gateway/production/metrics",
                scoped(CloudflareWorker, "api-gateway", "api-gateway", "/0123456789abcdef0123456789abcdef/workers/services/view/api-gateway"),
            ),
            (
                "https://dash.cloudflare.com/0123456789abcdef0123456789abcdef/pages/view/blog/deployments",
                scoped(CloudflarePages, "blog", "blog", "/0123456789abcdef0123456789abcdef/pages/view/blog"),
            ),
            ("https://dash.cloudflare.com/0123456789abcdef0123456789abcdef/workers-and-pages", None),
            (
                "https://console.firebase.google.com/project/my-app-prod/firestore/data",
                scoped(GoogleCloud, "my-app-prod", "my-app-prod", "/project/my-app-prod"),
            ),
            (
                "https://console.firebase.google.com/u/1/project/my-app-prod/overview",
                scoped(GoogleCloud, "my-app-prod", "my-app-prod", "/u/1/project/my-app-prod"),
            ),
            (
                "https://console.cloud.google.com/run/detail/us-central1/api/metrics?project=my-app-prod&authuser=1",
                Some((GoogleCloud, "my-app-prod".into(), "my-app-prod".into(), None)),
            ),
            ("https://console.cloud.google.com/welcome", None),
            (
                "https://sentry.io/organizations/acme/projects/web-frontend/?project=4504",
                scoped(Sentry, "acme/web-frontend", "web-frontend", "/organizations/acme/projects/web-frontend"),
            ),
            (
                "https://sentry.io/organizations/acme/issues/?project=4504123&statsPeriod=14d",
                Some((Sentry, "acme/4504123".into(), "4504123".into(), None)),
            ),
            (
                "https://sentry.io/acme/web-frontend/",
                scoped(Sentry, "acme/web-frontend", "web-frontend", "/acme/web-frontend"),
            ),
            (
                "https://acme.sentry.io/projects/web-frontend/getting-started/",
                scoped(Sentry, "web-frontend", "web-frontend", "/projects/web-frontend"),
            ),
            (
                "https://acme.sentry.io/issues/?project=4504123",
                Some((Sentry, "4504123".into(), "4504123".into(), None)),
            ),
            ("https://acme.sentry.io/issues/", None),
            ("https://sentry.io/settings/account/", None),
            ("https://example.com/apps/1234567890", None),
        ] {
            assert_eq!(key(url), want, "{url}");
        }
        assert!(known_host("acme.sentry.io") && known_host("vercel.com:443"));
        assert!(!known_host("sentry.io.evil.test") && !known_host("example.com"));
    }

    #[test]
    fn local_files_name_hosted_projects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".vercel")).unwrap();
        std::fs::create_dir_all(root.join("fastlane")).unwrap();
        std::fs::write(
            root.join(".vercel/project.json"),
            r#"{"projectId":"prj_123","orgId":"team_9","projectName":"marketing-site"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("wrangler.toml"),
            "name = \"api-gateway\"\nmain = \"src/index.ts\"\n[env.staging]\nname = \"api-staging\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".firebaserc"),
            r#"{"projects":{"default":"my-app-prod","staging":"my-app-staging"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".sentryclirc"),
            "[defaults]\norg=acme\nproject=web-frontend\n",
        )
        .unwrap();
        std::fs::write(
            root.join("fastlane/Appfile"),
            "app_identifier(\"com.acme.app\")\napple_id(\"dev@acme.com\")\n",
        )
        .unwrap();
        std::fs::write(
            root.join("fastlane/Fastfile"),
            "lane :beta do\n  upload_to_testflight(apple_id: \"1234567890\")\nend\n",
        )
        .unwrap();
        let keys = local_keys(root);
        for want in [
            (Kind::Vercel, "marketing-site"),
            (Kind::CloudflareWorker, "api-gateway"),
            (Kind::GoogleCloud, "my-app-prod"),
            (Kind::GoogleCloud, "my-app-staging"),
            (Kind::Sentry, "web-frontend"),
            (Kind::AppStoreApp, "1234567890"),
        ] {
            assert!(
                keys.contains(&(want.0, want.1.to_string())),
                "{want:?} in {keys:?}"
            );
        }
        assert!(!keys
            .iter()
            .any(|(_, v)| v == "api-staging" || v.contains('@')));
        let candidates = vec![
            (
                PathBuf::from("/w/other"),
                vec![(Kind::Vercel, "blog".to_string())],
            ),
            (root.to_path_buf(), keys),
        ];
        let found = |url: &str| {
            let u = url::Url::parse(url).unwrap();
            let p = page_project(
                u.host_str().unwrap(),
                u.path(),
                &u.query().map(|q| format!("?{q}")).unwrap_or_default(),
            )
            .unwrap();
            matches(&p, &candidates)
        };
        let here = vec![root.to_path_buf()];
        assert_eq!(found("https://vercel.com/acme/Marketing-Site"), here);
        assert_eq!(
            found("https://dash.cloudflare.com/abc/pages/view/api-gateway"),
            here
        );
        assert_eq!(
            found("https://console.cloud.google.com/home?project=my-app-staging"),
            here
        );
        assert_eq!(
            found("https://appstoreconnect.apple.com/apps/1234567890/testflight/ios"),
            here
        );
        assert_eq!(found("https://acme.sentry.io/projects/web-frontend/"), here);
        assert_eq!(
            found("https://vercel.com/acme/blog"),
            vec![PathBuf::from("/w/other")]
        );
        assert!(found("https://app.netlify.com/sites/marketing-site").is_empty());
        assert!(found("https://appstoreconnect.apple.com/apps/999999999").is_empty());
    }
}
