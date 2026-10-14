//! Scopes, origin patterns and target fingerprints for Inspect & Ask.
//!
//! A scope is an origin plus a `pathScope` of whole path segments. A page
//! matches a scope when the scope's path is a segment prefix of the page path:
//! `/admin` matches `/admin` and `/admin/orders`, never `/administrator`.
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MAX_SCOPE: usize = 200;
const MAX_PAGE_PATH: usize = 2000;

/// RFC 3986 `pchar` without `/`: what may appear inside one path segment.
fn segment_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@%".contains(&b)
}

/// `/` or `/seg[/seg…]`: no empty, `.` or `..` segment, no trailing slash,
/// no query or fragment.
pub(in crate::agent_panel) fn path_scope(input: &str) -> Result<String, String> {
    let invalid =
        || format!("Invalid pathScope {input:?}; expected \"/\" or \"/segment[/segment…]\"");
    if input == "/" {
        return Ok(input.into());
    }
    let Some(rest) = input.strip_prefix('/') else {
        return Err(invalid());
    };
    if input.len() > MAX_SCOPE
        || rest
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".." || !s.bytes().all(segment_char))
    {
        return Err(invalid());
    }
    Ok(input.into())
}

/// The page's path as `location.pathname` serializes it, with a trailing
/// slash removed (`/admin/` is the page `/admin`). Query and fragment are
/// refused, not stripped: the caller sends the path alone.
pub(in crate::agent_panel) fn page_path(input: &str) -> Result<String, String> {
    if !input.starts_with('/')
        || input.len() > MAX_PAGE_PATH
        || !input.bytes().all(|b| b == b'/' || segment_char(b))
    {
        return Err(
            "pagePath must be a URL path starting with '/' without query or fragment".into(),
        );
    }
    let trimmed = input.trim_end_matches('/');
    Ok(if trimmed.is_empty() { "/" } else { trimmed }.into())
}

/// Whether `scope` covers `page` on a segment boundary.
pub(in crate::agent_panel) fn applies(scope: &str, page: &str) -> bool {
    scope == "/"
        || page == scope
        || page
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Two scopes of one origin overlap when either covers the other.
pub(in crate::agent_panel) fn overlaps(a: &str, b: &str) -> bool {
    applies(a, b) || applies(b, a)
}

/// Every scope that covers `page`, broadest first: `/`, `/a`, `/a/b`.
pub(in crate::agent_panel) fn covering(page: &str) -> Vec<String> {
    let mut scopes = vec!["/".to_string()];
    let mut current = String::new();
    for segment in page.split('/').filter(|s| !s.is_empty()) {
        current.push('/');
        current.push_str(segment);
        scopes.push(current.clone());
    }
    scopes
}

/// What a binding, claim or card pair sends requests to.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::agent_panel) struct Target {
    pub root: PathBuf,
    pub source_roots: Vec<PathBuf>,
    pub session_id: Option<String>,
    pub session_cwd: Option<PathBuf>,
}

/// Lowercase hex SHA-256, first 32 characters, over
/// `origin \n pathScope \n root \n sourceRoots joined by \n \n sessionId \n sessionCwd`.
/// An absent session contributes empty strings. The host recomputes it from
/// live data whenever a request quotes one.
pub(in crate::agent_panel) fn fingerprint(origin: &str, scope: &str, target: &Target) -> String {
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let roots: Vec<String> = target.source_roots.iter().map(|p| text(p)).collect();
    let fields = [
        origin.to_string(),
        scope.to_string(),
        text(&target.root),
        roots.join("\n"),
        target.session_id.clone().unwrap_or_default(),
        target.session_cwd.as_deref().map(text).unwrap_or_default(),
    ];
    let digest = Sha256::digest(fields.join("\n").as_bytes());
    hex::encode(digest)[..32].to_string()
}

/// An origin in `.chrome-use/inspect.json`: exact, or `scheme://*.rest[:port]`
/// where `*` stands for exactly one DNS label.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::agent_panel) enum OriginPattern {
    Exact(String),
    Wildcard {
        scheme: String,
        /// `.preview.x.com`, leading dot included.
        suffix: String,
        port: Option<u16>,
    },
}

/// Placeholder that stands in for `*` while the rest is validated as an
/// ordinary origin.
const LABEL: &str = "wildcard-label";

impl OriginPattern {
    pub(in crate::agent_panel) fn parse(input: &str) -> Result<Self, String> {
        let Some((scheme, rest)) = input.split_once("://") else {
            return Err(format!("Invalid origin {input:?}"));
        };
        let Some(after) = rest.strip_prefix("*.") else {
            if input.contains('*') {
                return Err(format!(
                    "Origin {input:?}: '*' may only stand for the whole first DNS label"
                ));
            }
            return super::origin(input).map(Self::Exact);
        };
        if after.contains('*') {
            return Err(format!("Origin {input:?} may contain one '*' only"));
        }
        let probe = format!("{scheme}://{LABEL}.{after}");
        super::origin(&probe).map_err(|_| format!("Invalid wildcard origin {input:?}"))?;
        let url =
            url::Url::parse(&probe).map_err(|_| format!("Invalid wildcard origin {input:?}"))?;
        let host = url.host_str().unwrap_or_default();
        let suffix = host[LABEL.len()..].to_string();
        // `*.com` would claim a whole top-level domain.
        if !matches!(url.host(), Some(url::Host::Domain(_))) || suffix.matches('.').count() < 2 {
            return Err(format!(
                "Wildcard origin {input:?} must keep at least two labels after '*.'"
            ));
        }
        Ok(Self::Wildcard {
            scheme: url.scheme().into(),
            suffix,
            port: url.port(),
        })
    }

    /// `Some(false)` for an exact match, `Some(true)` for a wildcard match.
    pub(in crate::agent_panel) fn matches(&self, origin: &str) -> Option<bool> {
        match self {
            Self::Exact(exact) => (exact == origin).then_some(false),
            Self::Wildcard {
                scheme,
                suffix,
                port,
            } => {
                let url = url::Url::parse(origin).ok()?;
                let host = url.host_str()?;
                let label = host.strip_suffix(suffix.as_str())?;
                (url.scheme() == scheme
                    && url.port() == *port
                    && !label.is_empty()
                    && !label.contains('.'))
                .then_some(true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_match_on_segment_boundaries() {
        assert!(applies("/", "/anything"));
        assert!(applies("/admin", "/admin"));
        assert!(applies("/admin", "/admin/orders"));
        assert!(!applies("/admin", "/administrator"));
        assert!(!applies("/admin", "/"));
        assert!(!applies("/admin/orders", "/admin"));
        assert!(overlaps("/", "/admin"));
        assert!(overlaps("/admin/x", "/admin"));
        assert!(!overlaps("/admin", "/shop"));
        assert!(!overlaps("/admin", "/administrator"));
        assert_eq!(covering("/a/b"), ["/", "/a", "/a/b"]);
        assert_eq!(covering("/"), ["/"]);
    }

    #[test]
    fn scope_and_page_grammar() {
        for good in ["/", "/admin", "/a/b-c/d_e", "/%E4%B8%AD"] {
            assert_eq!(path_scope(good).unwrap(), good);
        }
        for bad in [
            "", "admin", "/admin/", "//", "/a//b", "/a/../b", "/./a", "/a?x", "/a#x", "/a b",
            "/a\\b",
        ] {
            assert!(path_scope(bad).is_err(), "{bad:?} accepted");
        }
        assert!(path_scope(&format!("/{}", "a".repeat(300))).is_err());
        assert_eq!(page_path("/admin/").unwrap(), "/admin");
        assert_eq!(page_path("/").unwrap(), "/");
        assert_eq!(page_path("//").unwrap(), "/");
        for bad in ["", "admin", "/a?x=1", "/a#f", "/a b", "/a\n"] {
            assert!(page_path(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn fingerprint_is_stable_and_covers_every_field() {
        let base = std::env::temp_dir();
        let target = Target {
            root: base.join("w"),
            source_roots: vec![base.join("w"), base.join("ui")],
            session_id: Some("claude-1".into()),
            session_cwd: Some(base.join("w")),
        };
        let a = fingerprint("https://x.test", "/", &target);
        assert_eq!(a.len(), 32);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(a, fingerprint("https://x.test", "/", &target.clone()));
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let expected = Sha256::digest(
            [
                "https://x.test".to_string(),
                "/".into(),
                text(&base.join("w")),
                format!("{}\n{}", text(&base.join("w")), text(&base.join("ui"))),
                "claude-1".into(),
                text(&base.join("w")),
            ]
            .join("\n"),
        );
        assert_eq!(a, hex::encode(expected)[..32]);
        let mut other = target.clone();
        other.source_roots.pop();
        assert_ne!(a, fingerprint("https://x.test", "/", &other));
        assert_ne!(a, fingerprint("https://x.test", "/admin", &target));
        let mut sessionless = target.clone();
        sessionless.session_id = None;
        assert_ne!(a, fingerprint("https://x.test", "/", &sessionless));
        // Known vector: no paths, so it is the same on every platform.
        let empty = Target {
            root: PathBuf::new(),
            source_roots: vec![],
            session_id: None,
            session_cwd: None,
        };
        assert_eq!(
            fingerprint("o", "/", &empty),
            hex::encode(Sha256::digest("o\n/\n\n\n\n"))[..32]
        );
    }

    #[test]
    fn wildcard_origins_cover_exactly_one_label_with_the_same_scheme_and_port() {
        let w = OriginPattern::parse("https://*.preview.x.com").unwrap();
        assert_eq!(w.matches("https://a.preview.x.com"), Some(true));
        assert_eq!(w.matches("https://a.b.preview.x.com"), None);
        assert_eq!(w.matches("https://preview.x.com"), None);
        assert_eq!(w.matches("http://a.preview.x.com"), None);
        assert_eq!(w.matches("https://a.preview.x.com:8443"), None);
        assert_eq!(w.matches("https://apreview.x.com"), None);
        let p = OriginPattern::parse("http://*.dev.x.com:8080").unwrap();
        assert_eq!(p.matches("http://a.dev.x.com:8080"), Some(true));
        assert_eq!(p.matches("http://a.dev.x.com"), None);
        let exact = OriginPattern::parse("http://localhost:5173").unwrap();
        assert_eq!(exact.matches("http://localhost:5173"), Some(false));
        assert_eq!(exact.matches("http://localhost:5174"), None);
        for bad in [
            "https://*.com",
            "https://a.*.x.com",
            "https://*x.y.com",
            "https://*.*.x.com",
            "https://*.x.com:443",
            "https://*.X.com",
            "https://*.x.com/",
            "ftp://*.x.y.com",
            "*.x.y.com",
        ] {
            assert!(OriginPattern::parse(bad).is_err(), "{bad} accepted");
        }
    }
}
