//! A site's project for panel conversations: the same bindings Inspect &
//! Ask keeps (one store, edited on the options page), read to preselect a new
//! conversation's project and written when the user chooses to remember
//! another one for the site.
use super::{origin, scope, Binding, BindingSource, Inspect};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

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
        let binding = self.locked(|| {
            let mut store = self.load()?;
            let current = store.binding(&origin, &path_scope).cloned();
            if current.as_ref().map(Binding::fingerprint) != p.replaces {
                return Err(
                    "This site's project changed since it was shown; nothing was saved. Try again."
                        .into(),
                );
            }
            let keep_session = current.as_ref().filter(|b| {
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
                source: BindingSource::Confirmed,
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

/// Every bound project with its site and when it was bound, for the panel's
/// list of recent project folders. Unreadable stores list nothing.
pub(in crate::agent_panel) fn bound_projects() -> Vec<(PathBuf, String, u64)> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let inspect = Inspect {
        root: home.join(".chrome-use").join("inspect"),
        sys: &super::Host,
        profile: None,
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
