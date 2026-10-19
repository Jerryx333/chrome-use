//! A site's project for panel conversations: the same bindings Inspect &
//! Ask keeps (one store, edited on the options page), read to preselect a new
//! conversation's project and written when the user chooses to remember
//! another one for the site. A page naming a repository goes to the recent
//! checkout of that repository unless the user bound the repository itself.
use super::{origin, scope, Binding, BindingSource, Inspect};
use crate::agent_panel::repos;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectParams {
    origin: String,
    page_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectBindParams {
    origin: String,
    #[serde(default = "super::root_scope")]
    path_scope: String,
    root: String,
    /// Fingerprint of the binding the user saw for this scope, or null when
    /// they saw none. A store changed since then is not overwritten.
    replaces: Option<String>,
    /// Another scope of the origin whose binding this one replaces (the
    /// options page changing a binding's scope): removed in the same write,
    /// its session kept.
    #[serde(default)]
    moves_from: Option<String>,
}

impl Inspect<'_> {
    /// The project bound to the page, if any, and whether it still exists.
    pub(super) fn project(&self, p: ProjectParams) -> Result<Value, String> {
        let origin = origin(&p.origin)?;
        let page = scope::page_path(&p.page_path)?;
        let store = self.load()?;
        Ok(match store.effective(&origin, &page) {
            Some(b) => json!({"binding":{
                "origin":b.origin,"pathScope":b.path_scope,"root":b.root,
                "exists":b.root.is_dir(),"fingerprint":b.fingerprint(),"sessionLabel":b.session_label,
            }}),
            None => json!({ "binding": null }),
        })
    }

    /// Remember `root` as the project of a site scope. The binding keeps its
    /// session only when that session works in the new project; a new root
    /// clears the scope's consents (Store::put).
    pub(super) fn project_bind(&self, p: ProjectBindParams) -> Result<Value, String> {
        let origin = origin(&p.origin)?;
        let path_scope = scope::path_scope(&p.path_scope)?;
        let root = super::resolve::absolute_dir(&p.root, "Project directory")?;
        let moves_from = p
            .moves_from
            .as_deref()
            .map(scope::path_scope)
            .transpose()?
            .filter(|from| *from != path_scope);
        let binding = self.locked(|| {
            let mut store = self.load()?;
            let current = store.binding(&origin, &path_scope).cloned();
            if current.as_ref().map(Binding::fingerprint) != p.replaces {
                return Err(
                    "This site's project changed since it was shown; nothing was saved. Try again."
                        .into(),
                );
            }
            let moved = match &moves_from {
                Some(from) => {
                    let old = store
                        .binding(&origin, from)
                        .cloned()
                        .ok_or("That binding is gone; nothing was saved.")?;
                    store
                        .bindings
                        .retain(|b| !(b.origin == origin && b.path_scope == *from));
                    Some(old)
                }
                None => None,
            };
            let keep_session = moved.as_ref().or(current.as_ref()).filter(|b| {
                b.session_cwd
                    .as_ref()
                    .is_some_and(|cwd| cwd.starts_with(&root) || root.starts_with(cwd))
            });
            let binding = store.put(Binding {
                origin: origin.clone(),
                path_scope: path_scope.clone(),
                root: root.clone(),
                source_roots: vec![root.clone()],
                session_id: keep_session.and_then(|b| b.session_id.clone()),
                session_cwd: keep_session.and_then(|b| b.session_cwd.clone()),
                session_label: keep_session.and_then(|b| b.session_label.clone()),
                source: moved
                    .as_ref()
                    .map_or(BindingSource::Confirmed, |b| b.source),
                evidence: None,
                consents: Default::default(),
                updated_at: super::super::conversation::now(),
            });
            self.persist(&store)?;
            Ok(binding)
        })?;
        Ok(json!({ "binding": binding.view() }))
    }
}

/// Why a page's project was chosen.
#[derive(Debug, PartialEq)]
enum Reason {
    /// A recent checkout's remote is the page's repository.
    Repository,
    /// A remembered site binding.
    Site,
}

/// The project for a page from its effective site binding (scope and root)
/// and the checkouts of the repository it names, best first. A binding at
/// or under the repository wins: the user chose a project for that very
/// repository. A broader one (the whole code host, an owner) does not
/// override the repository's own checkout, though it picks among several.
fn decide<'a>(
    binding: Option<(&str, &'a Path)>,
    found: &'a [repos::Match],
) -> Option<(&'a Path, Reason, Option<&'a repos::Match>)> {
    if let Some((bound, root)) = binding {
        let under_repo = found.first().is_none_or(|m| {
            scope::applies(&m.scope.to_ascii_lowercase(), &bound.to_ascii_lowercase())
        });
        if under_repo {
            return Some((root, Reason::Site, None));
        }
    }
    let chosen = found
        .iter()
        .find(|m| binding.is_some_and(|(_, root)| m.root == root))
        .or(found.first())?;
    Some((&chosen.root, Reason::Repository, Some(chosen)))
}

/// Recent checkouts read for one page.
const MAX_CANDIDATES: usize = 120;

/// What a new conversation on a page starts in, for the panel: the
/// suggested project and why, the repository the page names (for the
/// remember bar's scope), and every binding covering the page with its
/// fingerprint (what `inspectProjectBind` must be told it replaces).
/// `recent` is the panel's recent project folders, most recent first.
pub(in crate::agent_panel) fn suggest(
    origin_text: &str,
    page_path: &str,
    recent: &[PathBuf],
) -> Result<Value, String> {
    suggest_with(home_inspect().as_ref(), origin_text, page_path, recent)
}

fn suggest_with(
    inspect: Option<&Inspect>,
    origin_text: &str,
    page_path: &str,
    recent: &[PathBuf],
) -> Result<Value, String> {
    let origin = origin(origin_text)?;
    let page = scope::page_path(page_path)?;
    let host = origin.split_once("://").map_or("", |(_, host)| host);
    let store = inspect.map(Inspect::load).transpose()?.unwrap_or_default();
    let effective = store.effective(&origin, &page).filter(|b| b.root.is_dir());
    let candidates: Vec<_> = recent
        .iter()
        .take(MAX_CANDIDATES)
        .map(|root| (root.clone(), repos::remotes_of(root)))
        .collect();
    let found = repos::matches(host, &page, &candidates);
    let repository_scope = found
        .first()
        .map(|m| m.scope.clone())
        .or_else(|| repos::shape_scope(host, &page));
    let suggestion = decide(
        effective.map(|b| (b.path_scope.as_str(), b.root.as_path())),
        &found,
    )
    .map(|(root, reason, matched)| match reason {
        Reason::Repository => json!({
            "root": root, "reason": "repository",
            "pathScope": matched.map(|m| m.scope.clone()),
            "remote": matched.map(|m| m.remote.clone()),
        }),
        Reason::Site => json!({
            "root": root, "reason": "site",
            "pathScope": effective.map(|b| b.path_scope.clone()),
        }),
    });
    let bindings: Vec<Value> = store
        .bindings
        .iter()
        .filter(|b| b.origin == origin && scope::applies(&b.path_scope, &page))
        .map(|b| json!({"pathScope":b.path_scope,"root":b.root,"exists":b.root.is_dir(),"fingerprint":b.fingerprint()}))
        .collect();
    Ok(json!({
        "origin": origin,
        "repositoryScope": repository_scope,
        "codeHost": repos::known_host(host) || repository_scope.is_some(),
        "suggestion": suggestion,
        "bindings": bindings,
    }))
}

fn home_inspect() -> Option<Inspect<'static>> {
    Some(Inspect {
        root: dirs::home_dir()?.join(".chrome-use").join("inspect"),
        sys: &super::Host,
        profile: None,
    })
}

/// Every bound project with its site and when it was bound, for the panel's
/// list of recent project folders. Unreadable stores list nothing.
pub(in crate::agent_panel) fn bound_projects() -> Vec<(PathBuf, String, u64)> {
    let Some(inspect) = home_inspect() else {
        return Vec::new();
    };
    inspect
        .load()
        .map(|store| {
            store
                .bindings
                .iter()
                .map(|b| {
                    let site = if b.path_scope == "/" {
                        b.origin.clone()
                    } else {
                        format!("{}{}", b.origin, b.path_scope)
                    };
                    (b.root.clone(), site, b.updated_at)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{call, world, Fake, ORIGIN};
    use super::*;
    use std::fs;

    fn checkout(at: &Path, url: &str) -> PathBuf {
        fs::create_dir_all(at.join(".git")).unwrap();
        fs::write(
            at.join(".git/config"),
            format!("[remote \"origin\"]\n\turl = {url}\n"),
        )
        .unwrap();
        at.to_path_buf()
    }

    fn found(root: &str, scope: &str) -> repos::Match {
        repos::Match {
            root: root.into(),
            remote: "origin".into(),
            scope: scope.into(),
        }
    }

    #[test]
    fn a_repository_match_beats_broader_site_bindings_only() {
        let repo = [found("/w/image-use", "/leeguooooo/image-use")];
        let pick = |bound: Option<(&str, &str)>, found: &[repos::Match]| {
            decide(bound.map(|(s, r)| (s, Path::new(r))), found)
                .map(|(root, reason, _)| (root.to_string_lossy().to_string(), reason))
        };
        // github.com bound to another project: the repository's checkout wins.
        assert_eq!(
            pick(Some(("/", "/w/chatgpt-imagegen")), &repo),
            Some(("/w/image-use".into(), Reason::Repository))
        );
        assert_eq!(
            pick(Some(("/leeguooooo", "/w/chatgpt-imagegen")), &repo),
            Some(("/w/image-use".into(), Reason::Repository))
        );
        // The user bound this repository (or a path inside it) on purpose.
        for scope in [
            "/leeguooooo/image-use",
            "/LeeGuooooo/Image-Use",
            "/leeguooooo/image-use/pull",
        ] {
            assert_eq!(
                pick(Some((scope, "/w/image-use-old")), &repo),
                Some(("/w/image-use-old".into(), Reason::Site)),
                "{scope}"
            );
        }
        // No repository: the site binding as before; nothing at all: none.
        assert_eq!(
            pick(Some(("/", "/w/site")), &[]),
            Some(("/w/site".into(), Reason::Site))
        );
        assert_eq!(pick(None, &[]), None);
        assert_eq!(
            pick(None, &repo),
            Some(("/w/image-use".into(), Reason::Repository))
        );
        // Several checkouts: a broader binding naming one of them picks it.
        let two = [
            found("/w/image-use", "/leeguooooo/image-use"),
            found("/w/image-use-2", "/leeguooooo/image-use"),
        ];
        assert_eq!(
            pick(Some(("/", "/w/image-use-2")), &two),
            Some(("/w/image-use-2".into(), Reason::Repository))
        );
    }

    #[test]
    fn suggestions_read_recent_checkouts_and_list_covering_bindings() {
        let w = world();
        let sys = Fake::default();
        let base = w.project.parent().unwrap().to_path_buf();
        let imagegen = checkout(
            &base.join("chatgpt-imagegen"),
            "git@github.com:leeguooooo/chatgpt-imagegen.git",
        );
        let image_use = checkout(
            &base.join("image-use"),
            "https://github.com/leeguooooo/image-use.git",
        );
        let inspect = Inspect {
            root: w.data.clone(),
            sys: &sys,
            profile: None,
        };
        let recent = [imagegen.clone(), image_use.clone()];
        let github = "https://github.com";
        let bound = call(
            &w,
            &sys,
            "inspectProjectBind",
            json!({"origin": github, "root": imagegen, "replaces": null}),
        )
        .unwrap();
        let s = suggest_with(
            Some(&inspect),
            github,
            "/leeguooooo/image-use/pull/55",
            &recent,
        )
        .unwrap();
        assert_eq!(s["suggestion"]["root"], json!(image_use));
        assert_eq!(s["suggestion"]["reason"], "repository");
        assert_eq!(s["suggestion"]["pathScope"], "/leeguooooo/image-use");
        assert_eq!(s["repositoryScope"], "/leeguooooo/image-use");
        assert_eq!(s["codeHost"], true);
        assert_eq!(s["bindings"][0]["pathScope"], "/");
        assert_eq!(
            s["bindings"][0]["fingerprint"],
            bound["binding"]["fingerprint"]
        );
        // The origin-wide binding still holds where no repository matches.
        let other = suggest_with(Some(&inspect), github, "/someone/else", &recent).unwrap();
        assert_eq!(other["suggestion"]["root"], json!(imagegen));
        assert_eq!(other["suggestion"]["reason"], "site");
        assert_eq!(other["repositoryScope"], "/someone/else");
        // Binding the repository itself wins over the match.
        call(
            &w,
            &sys,
            "inspectProjectBind",
            json!({"origin": github, "pathScope": "/leeguooooo/image-use", "root": imagegen, "replaces": null}),
        )
        .unwrap();
        let s = suggest_with(Some(&inspect), github, "/leeguooooo/image-use", &recent).unwrap();
        assert_eq!(s["suggestion"]["reason"], "site");
        assert_eq!(s["suggestion"]["pathScope"], "/leeguooooo/image-use");
        assert_eq!(s["bindings"].as_array().unwrap().len(), 2);
        // A plain site: no repository, not a code host.
        let plain = suggest_with(Some(&inspect), ORIGIN, "/app/settings", &recent).unwrap();
        assert_eq!(plain["suggestion"], Value::Null);
        assert_eq!(plain["repositoryScope"], Value::Null);
        assert_eq!(plain["codeHost"], false);
        // A self-hosted server is recognized by the checkout alone.
        let api = checkout(&base.join("api"), "ssh://git@localhost:2222/team/api.git");
        let s = suggest_with(Some(&inspect), ORIGIN, "/team/api/whatever", &[api.clone()]).unwrap();
        assert_eq!(s["suggestion"]["root"], json!(api));
        assert_eq!(s["codeHost"], true);
        assert!(suggest_with(None, "nope", "/", &[]).is_err());
    }

    #[test]
    fn a_binding_can_move_to_another_scope_in_one_write() {
        let w = world();
        let sys = Fake::default();
        let bound = call(
            &w,
            &sys,
            "inspectProjectBind",
            json!({"origin": ORIGIN, "root": w.project, "replaces": null}),
        )
        .unwrap();
        let moved = call(
            &w,
            &sys,
            "inspectProjectBind",
            json!({"origin": ORIGIN, "pathScope": "/app", "root": w.project, "replaces": null, "movesFrom": "/"}),
        )
        .unwrap();
        assert_eq!(moved["binding"]["pathScope"], "/app");
        let list = call(&w, &sys, "inspectBindings", json!({})).unwrap();
        let scopes: Vec<_> = list["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["pathScope"].clone())
            .collect();
        assert_eq!(scopes, [json!("/app")]);
        // Moving from a scope that is gone saves nothing.
        assert!(call(
            &w,
            &sys,
            "inspectProjectBind",
            json!({"origin": ORIGIN, "pathScope": "/b", "root": w.project, "replaces": null, "movesFrom": "/"}),
        )
        .is_err());
        assert_ne!(
            bound["binding"]["fingerprint"],
            moved["binding"]["fingerprint"]
        );
    }
}
