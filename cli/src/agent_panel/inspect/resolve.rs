//! Matching a page to a project root and session (`inspectResolve`), and the
//! writes that follow a card: `inspectBind` and `inspectClaimDecide`.
//!
//! Only locally verifiable facts make a target: the user's click, a session's
//! own `inspect link`, a tracked project config and a loopback port's owner.
//! Page data (source paths, revision) only ranks and annotates candidates.
use super::{
    canonical_dir, config, facts, is_loopback, line, origin, scope, session_id, source_hint,
    BindParams, Binding, BindingSource, ClaimDecideParams, Evidence, Facts, Inspect, Relation,
    ResolveParams, Session, SourceRoot, TabTarget, Target, CANDIDATE_LIMIT, MAX_SOURCE_PATHS,
};
use facts::Worktree;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const MAX_WORKTREES: usize = 16;
const MAX_CONFIG_FILES: usize = 50;
const MAX_PAIRS: usize = 48;

/// Evidence kinds, strongest first.
const EVIDENCE: [&str; 5] = [
    "config",
    "config-wildcard",
    "dev-server-port",
    "previous-binding",
    "ocs-session",
];

/// A live session whose working directory still exists.
#[derive(Clone)]
pub(super) struct Live {
    pub session: Session,
    pub cwd: PathBuf,
}

pub(super) fn live(sessions: &[Session]) -> Vec<Live> {
    sessions
        .iter()
        .filter_map(|s| {
            canonical_dir(&s.cwd).map(|cwd| Live {
                session: s.clone(),
                cwd,
            })
        })
        .collect()
}

/// The explicit or default port of a loopback origin.
pub(super) fn loopback_port(origin: &str) -> Option<u16> {
    if !is_loopback(origin) {
        return None;
    }
    url::Url::parse(origin).ok()?.port_or_known_default()
}

/// A canonical, existing, absolute directory named by a request.
pub(super) fn absolute_dir(raw: &str, what: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err(format!("{what} path is invalid"));
    }
    if !Path::new(raw).is_absolute() {
        return Err(format!("{what} must be an absolute path"));
    }
    let dir =
        canonical_dir(Path::new(raw)).ok_or_else(|| format!("{what} {raw:?} does not exist"))?;
    if dir.to_string_lossy().chars().any(char::is_control) {
        return Err(format!("{what} path is invalid"));
    }
    Ok(dir)
}

#[derive(Clone)]
struct Candidate {
    root: PathBuf,
    worktree: Worktree,
    evidence: Vec<&'static str>,
    /// From a matching config: its scope, source roots and file.
    scope: Option<String>,
    source_roots: Vec<PathBuf>,
    config: Option<(PathBuf, bool)>,
    matched: usize,
    relation: Option<Relation>,
}

impl Candidate {
    fn rank(&self) -> usize {
        self.evidence
            .iter()
            .filter_map(|e| EVIDENCE.iter().position(|k| k == e))
            .min()
            .unwrap_or(EVIDENCE.len())
    }
    /// What ranking compares; equal keys are a tie.
    fn key(&self) -> (usize, std::cmp::Reverse<usize>, u8) {
        (
            self.rank(),
            std::cmp::Reverse(self.matched),
            self.relation
                .map_or(Relation::Unknown.rank(), Relation::rank),
        )
    }
    /// Config or dev-server evidence: the only kinds that may preselect.
    fn local_evidence(&self) -> bool {
        self.rank() <= 2
    }
}

fn session_info(s: &Session) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

/// Configs found in the known worktrees, and what could not be read.
pub(super) struct Discovered {
    pub configs: Vec<config::Config>,
    pub errors: Vec<Value>,
}

/// Read the tracked `.chrome-use/inspect.json` files of these worktrees, at
/// most `MAX_CONFIG_FILES` in all.
pub(super) fn discover(facts: &Facts, worktrees: &[PathBuf]) -> Discovered {
    let mut found = Discovered {
        configs: Vec::new(),
        errors: Vec::new(),
    };
    let mut files = 0;
    for top in worktrees {
        for rel in facts.configs(top) {
            files += 1;
            if files > MAX_CONFIG_FILES {
                facts.budget.skip(format!(
                    "project configs beyond the first {MAX_CONFIG_FILES} (in {})",
                    top.display()
                ));
                break;
            }
            match config::load(top, &rel) {
                Ok(c) => found.configs.push(c),
                Err(reason) => found
                    .errors
                    .push(json!({"path": top.join(&rel), "reason": reason})),
            }
        }
    }
    found
}

impl Inspect<'_> {
    pub(super) fn resolve(&self, p: ResolveParams) -> Result<Value, String> {
        let origin = origin(&p.origin)?;
        let page = scope::page_path(&p.page_path)?;
        if p.source_paths.len() > MAX_SOURCE_PATHS {
            return Err(format!("At most {MAX_SOURCE_PATHS} source paths"));
        }
        let hints = p
            .source_paths
            .iter()
            .map(|s| source_hint(s))
            .collect::<Result<Vec<_>, _>>()?;
        // Page data: a malformed revision is ignored, never passed to git.
        let revision = p.revision.as_deref().and_then(facts::revision);
        let port = loopback_port(&origin);
        if let (Some(asked), true) = (p.port, is_loopback(&origin)) {
            if Some(asked) != port {
                return Err("port does not match the page origin".into());
            }
        }
        let store = self.load()?;
        let (ocs, roster) = self.roster();
        let roster_error = roster.as_ref().err().cloned();
        let live = live(roster.as_deref().unwrap_or_default());
        let facts = Facts::new(self.sys);
        let claims: Vec<Value> = store
            .pending_claims
            .iter()
            .filter(|c| c.origin == origin && scope::applies(&c.path_scope, &page))
            .map(|c| store.claim_view(c))
            .collect();
        let mut out = json!({
            "platform": std::env::consts::OS,
            "ocs": match &ocs {
                Some(o) => json!({"installed": true, "path": o.path, "version": o.version}),
                None => json!({"installed": false}),
            },
            "pendingClaims": claims,
        });
        if let (Some(error), Some(_)) = (&roster_error, &ocs) {
            out["rosterError"] = error.clone().into();
        }
        let effective = store.effective(&origin, &page).cloned();
        if let Some(b) = &effective {
            let reason = self.stale_reason(&facts, b, &live);
            out["binding"] = b.view();
            out["binding"]["session"] = b
                .session_id
                .as_ref()
                .and_then(|id| live.iter().find(|l| &l.session.id == id))
                .map_or(Value::Null, |l| session_info(&l.session));
            match reason {
                None => {
                    out["state"] = "bound".into();
                    out["sessions"] = live
                        .iter()
                        .filter(|l| facts.related(&l.cwd, &b.root))
                        .map(|l| session_info(&l.session))
                        .collect::<Vec<_>>()
                        .into();
                    return Ok(finish(out, &facts));
                }
                Some(reason) => {
                    out["state"] = "needsConfirm".into();
                    out["reason"] = reason.into();
                }
            }
        } else {
            out["state"] = "unbound".into();
        }

        // ---- candidates
        let mut worktrees: Vec<PathBuf> = Vec::new();
        let mut note = |dir: &Path| {
            if let Worktree::Git(top) = facts.worktree(dir) {
                if !worktrees.contains(&top) {
                    worktrees.push(top);
                }
            }
        };
        for l in &live {
            note(&l.cwd);
        }
        for b in &store.bindings {
            if let Some(root) = canonical_dir(&b.root) {
                note(&root);
            }
        }
        if worktrees.len() > MAX_WORKTREES {
            facts.budget.skip(format!(
                "{} worktrees beyond the first {MAX_WORKTREES}",
                worktrees.len() - MAX_WORKTREES
            ));
            worktrees.truncate(MAX_WORKTREES);
        }
        let discovered = discover(&facts, &worktrees);
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut add = |root: PathBuf,
                       evidence: &'static str,
                       from: Option<(&config::Config, config::SiteMatch)>| {
            let i = match candidates.iter().position(|c| c.root == root) {
                Some(i) => i,
                None => {
                    candidates.push(Candidate {
                        worktree: facts.worktree(&root),
                        source_roots: vec![root.clone()],
                        root,
                        evidence: Vec::new(),
                        scope: None,
                        config: None,
                        matched: 0,
                        relation: None,
                    });
                    candidates.len() - 1
                }
            };
            let c = &mut candidates[i];
            if !c.evidence.contains(&evidence) {
                c.evidence.push(evidence);
            }
            if let Some((config, site)) = from {
                // The longest scope, then an exact origin, wins within a root.
                let better = match (&c.scope, &c.config) {
                    (Some(s), Some((_, wild))) => {
                        (site.path_scope.len(), !site.wildcard) > (s.len(), !wild)
                    }
                    _ => true,
                };
                if better {
                    c.scope = Some(site.path_scope);
                    c.source_roots = config.source_roots.clone();
                    c.config = Some((config.path.clone(), site.wildcard));
                }
            }
        };
        for config in &discovered.configs {
            if let Some(site) = config.site_for(&origin, &page) {
                let kind = if site.wildcard {
                    "config-wildcard"
                } else {
                    "config"
                };
                add(config.app_root.clone(), kind, Some((config, site)));
            }
        }
        if let Some(owner) = port.and_then(|port| facts.port_owner(port)) {
            add(owner, "dev-server-port", None);
        }
        for b in &store.bindings {
            if let Some(root) = canonical_dir(&b.root) {
                add(root, "previous-binding", None);
            }
        }
        for l in &live {
            let root = match facts.worktree(&l.cwd) {
                Worktree::Git(top) => top,
                _ => l.cwd.clone(),
            };
            add(root, "ocs-session", None);
        }
        for c in &mut candidates {
            c.evidence
                .sort_by_key(|e| EVIDENCE.iter().position(|k| k == e));
            let roots = SourceRoot::of(&facts, &c.root, &c.source_roots);
            c.matched = hints.iter().filter(|h| h.matches(&roots)).count();
            c.relation = revision.map(|rev| match &c.worktree {
                Worktree::Git(top) => facts.relation(top, rev),
                _ => Relation::Unknown,
            });
        }
        candidates.sort_by(|a, b| a.key().cmp(&b.key()).then(a.root.cmp(&b.root)));
        candidates.truncate(CANDIDATE_LIMIT);

        let default_scope = effective
            .as_ref()
            .map_or_else(|| "/".to_string(), |b| b.path_scope.clone());
        let mut pairs: Vec<Value> = Vec::new();
        let mut preselected = None;
        for (i, c) in candidates.iter().enumerate() {
            let path_scope = c.scope.clone().unwrap_or_else(|| default_scope.clone());
            let related: Vec<&Live> = live
                .iter()
                .filter(|l| facts.related(&l.cwd, &c.root))
                .collect();
            let tied = candidates
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && other.key() == c.key());
            if i == 0 && c.local_evidence() && related.len() == 1 && !tied {
                preselected = Some(pairs.len());
            }
            let sessions: Vec<Option<&Live>> = if related.is_empty() {
                vec![None]
            } else {
                related.into_iter().map(Some).collect()
            };
            for l in sessions {
                if pairs.len() >= MAX_PAIRS {
                    break;
                }
                pairs.push(pair(&origin, &path_scope, c, l));
            }
        }
        let mut card = json!({ "pairs": pairs });
        if let Some(i) = preselected {
            card["preselected"] = card["pairs"][i].clone();
        }
        out["card"] = card;
        let shown_root = effective
            .as_ref()
            .map(|b| b.root.clone())
            .or_else(|| candidates.first().map(|c| c.root.clone()));
        out["sessions"] = shown_root
            .map(|root| {
                live.iter()
                    .filter(|l| facts.related(&l.cwd, &root))
                    .map(|l| session_info(&l.session))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into();
        if !discovered.errors.is_empty() {
            out["configErrors"] = discovered.errors.into();
        }
        Ok(finish(out, &facts))
    }

    /// Why an effective binding cannot be used as is, or `None` when its
    /// session is live in the same directory and its dev-server evidence
    /// still holds.
    pub(super) fn stale_reason(
        &self,
        facts: &Facts,
        b: &Binding,
        live: &[Live],
    ) -> Option<&'static str> {
        if canonical_dir(&b.root).as_ref() != Some(&b.root) {
            return Some("rootMissing");
        }
        let Some(session) = b
            .session_id
            .as_ref()
            .and_then(|id| live.iter().find(|l| &l.session.id == id))
        else {
            return Some("sessionEnded");
        };
        if Some(&session.cwd) != b.session_cwd.as_ref() {
            return Some("sessionMoved");
        }
        if !port_holds(facts, b) {
            return Some("portMoved");
        }
        None
    }

    pub(super) fn bind(&self, p: BindParams) -> Result<Value, String> {
        let origin = origin(&p.origin)?;
        let root = absolute_dir(&p.root, "Project directory")?;
        let mut label = p
            .session_label
            .as_deref()
            .map(|l| line(l, 120))
            .filter(|l| !l.is_empty());
        let Some(fingerprint) = p.fingerprint else {
            return self.bind_legacy(
                &origin,
                root,
                p.session_id,
                label,
                p.path_scope.is_some() || p.source_roots.is_some(),
            );
        };
        let path_scope = scope::path_scope(p.path_scope.as_deref().unwrap_or("/"))?;
        let id = p
            .session_id
            .ok_or("A confirmed binding needs the session from the card")?;
        session_id(&id)?;
        let requested = match &p.source_roots {
            Some(roots) => roots
                .iter()
                .map(|r| absolute_dir(r, "Source root"))
                .collect::<Result<Vec<_>, _>>()?,
            None => vec![root.clone()],
        };
        let (ocs, roster) = self.roster();
        if ocs.is_none() {
            return Err("ocs is not installed, so the session cannot be confirmed".into());
        }
        let live = live(&roster?);
        let facts = Facts::new(self.sys);
        let rebuilt = live_pair(
            &facts,
            &origin,
            std::slice::from_ref(&path_scope),
            &root,
            &requested,
            &id,
            &live,
        )?;
        if rebuilt.fingerprint != fingerprint {
            return Err("The project or session changed since the card was shown; nothing was saved. Reopen the picker.".into());
        }
        if label.is_none() {
            label = rebuilt.label.clone();
        }
        let binding = self.locked(|| {
            let mut store = self.load()?;
            let binding = store.put(Binding {
                origin: origin.clone(),
                path_scope: path_scope.clone(),
                root: root.clone(),
                source_roots: rebuilt.target.source_roots.clone(),
                session_id: Some(id.clone()),
                session_cwd: rebuilt.target.session_cwd.clone(),
                session_label: label.clone(),
                source: BindingSource::Confirmed,
                evidence: rebuilt.evidence.clone(),
                consents: Default::default(),
                updated_at: super::super::conversation::now(),
            });
            self.persist(&store)?;
            Ok(binding)
        })?;
        Ok(json!({ "binding": binding.view() }))
    }

    /// Extension 0.5.56 binds whole origins and knows nothing of scopes.
    fn bind_legacy(
        &self,
        origin: &str,
        root: PathBuf,
        session: Option<String>,
        label: Option<String>,
        scoped: bool,
    ) -> Result<Value, String> {
        if scoped {
            return Err("pathScope and sourceRoots need the card's fingerprint".into());
        }
        let mut session_cwd = None;
        if let Some(id) = &session {
            session_id(id)?;
            let (ocs, roster) = self.roster();
            if ocs.is_none() {
                return Err("ocs is not installed, so the session cannot be confirmed".into());
            }
            let session = roster?
                .into_iter()
                .find(|s| &s.id == id)
                .ok_or("That session is no longer running; pick another")?;
            let cwd = canonical_dir(&session.cwd)
                .ok_or("That session's working directory no longer exists")?;
            if !(cwd.starts_with(&root) || root.starts_with(&cwd)) {
                return Err("That session works in a different directory than the project".into());
            }
            session_cwd = Some(cwd);
        }
        let binding = self.locked(|| {
            let mut store = self.load()?;
            if store.needs_upgrade(origin) {
                return Err("upgradeRequired: this site has path scopes or a pending claim; update the chrome-use extension to change it".into());
            }
            let binding = store.put(Binding {
                origin: origin.to_string(),
                path_scope: "/".into(),
                source_roots: vec![root.clone()],
                root: root.clone(),
                session_id: session.clone(),
                session_cwd: session_cwd.clone(),
                session_label: label.clone(),
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

    /// Accept or reject a pending claim. The card is current only when the
    /// claim is still pending and the complete set of overlapping bindings
    /// with another target equals what the card showed; the check, the
    /// decision and the write happen under one lock.
    pub(super) fn decide(&self, p: ClaimDecideParams) -> Result<Value, String> {
        super::claim_id(&p.claim_id)?;
        let mut expected = p.expected_affects.clone();
        for f in &expected {
            if f.len() != 32 || !f.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("expectedAffects must hold fingerprints".into());
            }
        }
        expected.sort();
        expected.dedup();
        const GONE: &str = "stale: this claim is no longer pending; reopen the picker";
        // A claim never changes while pending, so its claimant can be checked
        // before the lock without holding the lock across `ocs`.
        let claim = self
            .load()?
            .pending_claims
            .into_iter()
            .find(|c| c.claim_id == p.claim_id)
            .ok_or(GONE)?;
        if p.accept {
            let (ocs, roster) = self.roster();
            if ocs.is_none() {
                return Err(
                    "ocs is not installed, so the claiming session cannot be confirmed".into(),
                );
            }
            let running = live(&roster?)
                .into_iter()
                .any(|l| l.session.id == claim.session_id && l.cwd == claim.session_cwd);
            if !running {
                return Err("The claiming session ended or moved; nothing was changed".into());
            }
        }
        let binding = self.locked(|| {
            let mut store = self.load()?;
            let i = store
                .pending_claims
                .iter()
                .position(|c| c.claim_id == p.claim_id)
                .ok_or(GONE)?;
            let claim = store.pending_claims[i].clone();
            let mut current: Vec<String> = store
                .conflicting(&claim.origin, &claim.path_scope, &claim.target())
                .iter()
                .map(|b| b.fingerprint())
                .collect();
            current.sort();
            if current != expected {
                return Err("stale: the bindings this claim affects changed since the card was shown; reopen the picker".into());
            }
            store.pending_claims.remove(i);
            let binding = p.accept.then(|| {
                store.put(Binding {
                    origin: claim.origin.clone(),
                    path_scope: claim.path_scope.clone(),
                    root: claim.root.clone(),
                    source_roots: claim.source_roots.clone(),
                    session_id: Some(claim.session_id.clone()),
                    session_cwd: Some(claim.session_cwd.clone()),
                    session_label: claim.session_label.clone(),
                    source: BindingSource::Linked,
                    evidence: claim.evidence.clone(),
                    consents: Default::default(),
                    updated_at: super::super::conversation::now(),
                })
            });
            self.persist(&store)?;
            Ok(binding)
        })?;
        let mut out = json!({ "decided": if p.accept { "accepted" } else { "rejected" } });
        if let Some(b) = binding {
            out["binding"] = b.view();
        }
        Ok(out)
    }

    /// Re-validate a tab-only target against live data. `None` means the
    /// target no longer matches what its card showed.
    pub(super) fn tab_target(
        &self,
        facts: &Facts,
        origin: &str,
        page: &str,
        t: &TabTarget,
        live: &[Live],
    ) -> Result<Option<Target>, String> {
        session_id(&t.session_id)?;
        let Ok(root) = absolute_dir(&t.root, "Project directory") else {
            return Ok(None);
        };
        if Path::new(&t.root) != root {
            return Ok(None);
        }
        let mut requested = Vec::new();
        for r in &t.source_roots {
            match absolute_dir(r, "Source root") {
                Ok(dir) if Path::new(r) == dir => requested.push(dir),
                _ => return Ok(None),
            }
        }
        let scopes = scope::covering(page);
        let Ok(rebuilt) = live_pair(
            facts,
            origin,
            &scopes,
            &root,
            &requested,
            &t.session_id,
            live,
        ) else {
            return Ok(None);
        };
        let cwd_matches = rebuilt
            .target
            .session_cwd
            .as_deref()
            .is_some_and(|c| Path::new(&t.session_cwd) == c);
        if !cwd_matches || !rebuilt.fingerprints.contains(&t.fingerprint) {
            return Ok(None);
        }
        if !foreign_port_free(facts, origin, &root) {
            return Ok(None);
        }
        Ok(Some(rebuilt.target))
    }
}

/// A card pair rebuilt from live data.
pub(super) struct Rebuilt {
    pub target: Target,
    pub evidence: Option<Evidence>,
    pub label: Option<String>,
    /// The first entry of `fingerprints`.
    pub fingerprint: String,
    /// One per scope the pair was rebuilt for.
    pub fingerprints: Vec<String>,
}

/// Rebuild a pair from live data: the session must run in a directory
/// related to `root` (same worktree, one containing the other), and the
/// source roots must be exactly `[root]` or exactly what a project config
/// at `root` names for this origin and one of `scopes`. Never wider.
pub(super) fn live_pair(
    facts: &Facts,
    origin: &str,
    scopes: &[String],
    root: &Path,
    requested: &[PathBuf],
    id: &str,
    live: &[Live],
) -> Result<Rebuilt, String> {
    let session = live
        .iter()
        .find(|l| l.session.id == id)
        .ok_or("That session is no longer running; pick another")?;
    if !facts.related(&session.cwd, root) {
        return Err(
            "That session works in a different directory or worktree than the project".into(),
        );
    }
    let configs: Vec<config::Config> = match facts.worktree(root) {
        Worktree::Git(top) => discover(facts, &[top])
            .configs
            .into_iter()
            .filter(|c| c.app_root == root)
            .collect(),
        _ => Vec::new(),
    };
    let mut fingerprints = Vec::new();
    let mut evidence = None;
    let mut source_roots = None;
    for path_scope in scopes {
        let from_config = configs
            .iter()
            .find_map(|c| c.site_with_scope(origin, path_scope).map(|m| (c, m)));
        let allowed = requested == [root.to_path_buf()]
            || from_config
                .as_ref()
                .is_some_and(|(c, _)| c.source_roots == requested);
        if !allowed {
            continue;
        }
        let target = Target {
            root: root.to_path_buf(),
            source_roots: requested.to_vec(),
            session_id: Some(id.to_string()),
            session_cwd: Some(session.cwd.clone()),
        };
        fingerprints.push(scope::fingerprint(origin, path_scope, &target));
        if source_roots.is_none() {
            source_roots = Some(requested.to_vec());
            evidence = from_config.map(|(c, m)| Evidence::Config {
                path: c.path.clone(),
                wildcard: m.wildcard,
            });
        }
    }
    let source_roots = source_roots
        .ok_or("Those source roots are not the project's own or its config's; nothing was saved")?;
    // Dev-server evidence is recorded only when it holds now.
    if let Some(port) = loopback_port(origin) {
        if let Some(owner) = facts.port_owner(port) {
            if facts.related(&owner, root) {
                evidence = Some(Evidence::Port { port, cwd: owner });
            }
        }
    }
    Ok(Rebuilt {
        target: Target {
            root: root.to_path_buf(),
            source_roots,
            session_id: Some(id.to_string()),
            session_cwd: Some(session.cwd.clone()),
        },
        evidence,
        label: session.session.name.clone(),
        fingerprint: fingerprints[0].clone(),
        fingerprints,
    })
}

/// A binding with dev-server evidence holds only while the loopback port's
/// owner runs in the bound worktree, at, inside or above the root.
pub(super) fn port_holds(facts: &Facts, b: &Binding) -> bool {
    match &b.evidence {
        Some(Evidence::Port { port, .. }) if is_loopback(&b.origin) => facts
            .port_owner(*port)
            .is_some_and(|owner| facts.related(&owner, &b.root)),
        _ => true,
    }
}

/// A loopback page served by a dev server from another worktree must not
/// reach this root. An owner outside any worktree (a container, a proxy) is
/// no evidence either way; an owner that could not be checked is refused.
pub(super) fn foreign_port_free(facts: &Facts, origin: &str, root: &Path) -> bool {
    let Some(port) = loopback_port(origin) else {
        return true;
    };
    let before = facts.skipped().len();
    let owner = facts.port_owner(port);
    if facts.skipped().len() != before {
        return false;
    }
    match owner {
        None => true,
        Some(owner) => facts.related(&owner, root) || facts.worktree(&owner) == Worktree::Plain,
    }
}

fn pair(origin: &str, path_scope: &str, c: &Candidate, l: Option<&Live>) -> Value {
    let target = Target {
        root: c.root.clone(),
        source_roots: c.source_roots.clone(),
        session_id: l.map(|l| l.session.id.clone()),
        session_cwd: l.map(|l| l.cwd.clone()),
    };
    let mut v = json!({
        "origin": origin,
        "pathScope": path_scope,
        "root": c.root,
        "sourceRoots": c.source_roots,
        "sessionId": target.session_id,
        "sessionCwd": target.session_cwd,
        "evidence": c.evidence,
        "matchedPaths": c.matched,
        "fingerprint": scope::fingerprint(origin, path_scope, &target),
    });
    if let Some(l) = l {
        v["kind"] = l.session.kind.clone().into();
        if let Some(name) = &l.session.name {
            v["sessionLabel"] = name.clone().into();
        }
        if let Some(status) = &l.session.status {
            v["status"] = status.clone().into();
        }
    }
    if let Some(r) = c.relation {
        v["revision"] = r.view();
    }
    v
}

fn finish(mut out: Value, facts: &Facts) -> Value {
    let skipped = facts.skipped();
    if !skipped.is_empty() {
        out["skipped"] = skipped.into();
    }
    out
}
