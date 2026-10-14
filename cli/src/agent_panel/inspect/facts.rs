//! Local facts behind a match: git worktrees, dev-server port owners and
//! revision relations. Every git and lsof call made while answering one
//! request shares one budget; a call that does not fit, or that times out, is
//! reported as skipped and treated as unknown, never guessed.
use super::{canonical_dir, System};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) const TOTAL: Duration = Duration::from_secs(8);
pub(super) const PER_CALL: Duration = Duration::from_secs(3);

/// What one bounded external call found.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Probe<T> {
    Found(T),
    /// The call ran and answered "nothing" (not a git directory, no listener).
    Absent,
    TimedOut,
}

pub(super) struct Budget {
    total: Duration,
    used: Cell<Duration>,
    skipped: RefCell<Vec<String>>,
}

impl Budget {
    pub(super) fn new(total: Duration) -> Self {
        Self {
            total,
            used: Cell::new(Duration::ZERO),
            skipped: RefCell::default(),
        }
    }

    pub(super) fn skip(&self, what: String) {
        let mut skipped = self.skipped.borrow_mut();
        if !skipped.contains(&what) {
            skipped.push(what);
        }
    }

    /// Run `call` with the time left for it (at most `PER_CALL`), or skip it
    /// when the budget is spent. A timed-out call is charged its full slice.
    pub(super) fn probe<T>(
        &self,
        what: impl FnOnce() -> String,
        call: impl FnOnce(Duration) -> Probe<T>,
    ) -> Option<T> {
        let remaining = self.total.saturating_sub(self.used.get());
        if remaining.is_zero() {
            self.skip(format!("{} (time budget spent)", what()));
            return None;
        }
        let slice = remaining.min(PER_CALL);
        let started = Instant::now();
        let result = call(slice);
        let mut spent = started.elapsed();
        if matches!(result, Probe::TimedOut) {
            spent = spent.max(slice);
        }
        self.used.set(self.used.get() + spent);
        match result {
            Probe::Found(value) => Some(value),
            Probe::Absent => None,
            Probe::TimedOut => {
                self.skip(format!("{} (timed out)", what()));
                None
            }
        }
    }

    pub(super) fn skipped(&self) -> Vec<String> {
        self.skipped.borrow().clone()
    }
}

impl<T> Probe<T> {
    fn timed_out(&self) -> bool {
        matches!(self, Probe::TimedOut)
    }
}

/// Which worktree a directory belongs to.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Worktree {
    /// Canonical `git rev-parse --show-toplevel`.
    Git(PathBuf),
    /// Not inside a git worktree.
    Plain,
    /// The probe was skipped or timed out.
    Unknown,
}

/// The page's deployed revision compared with a worktree's HEAD.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Relation {
    Same,
    /// HEAD contains the deployed commit plus `n` more.
    Ahead(u32),
    /// The deployed commit is `n` commits past HEAD.
    Behind(u32),
    Diverged {
        ahead: u32,
        behind: u32,
    },
    Unknown,
}

impl Relation {
    /// `git rev-list --left-right --count <rev>...HEAD` prints
    /// `<only in rev>\t<only in HEAD>`.
    pub(super) fn from_counts(output: &str) -> Self {
        let mut counts = output.split_whitespace().map(str::parse::<u32>);
        match (counts.next(), counts.next(), counts.next()) {
            (Some(Ok(0)), Some(Ok(0)), None) => Self::Same,
            (Some(Ok(0)), Some(Ok(ahead)), None) => Self::Ahead(ahead),
            (Some(Ok(behind)), Some(Ok(0)), None) => Self::Behind(behind),
            (Some(Ok(behind)), Some(Ok(ahead)), None) => Self::Diverged { ahead, behind },
            _ => Self::Unknown,
        }
    }

    /// Lower ranks first: a worktree containing the deployed commit wins.
    pub(super) fn rank(self) -> u8 {
        match self {
            Self::Same => 0,
            Self::Ahead(_) => 1,
            Self::Behind(_) => 2,
            Self::Diverged { .. } => 3,
            Self::Unknown => 4,
        }
    }

    pub(super) fn view(self) -> Value {
        match self {
            Self::Same => json!({"relation": "same", "ahead": 0, "behind": 0}),
            Self::Ahead(n) => json!({"relation": "ahead", "ahead": n, "behind": 0}),
            Self::Behind(n) => json!({"relation": "behind", "ahead": 0, "behind": n}),
            Self::Diverged { ahead, behind } => {
                json!({"relation": "diverged", "ahead": ahead, "behind": behind})
            }
            Self::Unknown => json!({"relation": "unknown"}),
        }
    }
}

/// `^[0-9a-f]{7,40}$`: anything else is never passed to git.
pub(super) fn revision(text: &str) -> Option<&str> {
    (7..=40).contains(&text.len()).then_some(())?;
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        .then_some(text)
}

/// Memoized, budgeted facts for one request.
pub(super) struct Facts<'a> {
    sys: &'a dyn System,
    pub(super) budget: Budget,
    worktrees: RefCell<HashMap<PathBuf, Worktree>>,
    owners: RefCell<HashMap<u16, Option<PathBuf>>>,
    relations: RefCell<HashMap<PathBuf, Relation>>,
}

impl<'a> Facts<'a> {
    pub(super) fn new(sys: &'a dyn System) -> Self {
        Self {
            sys,
            budget: Budget::new(TOTAL),
            worktrees: RefCell::default(),
            owners: RefCell::default(),
            relations: RefCell::default(),
        }
    }

    /// The worktree of a canonical directory. A skipped probe is not
    /// remembered as an answer, but is not retried within this request
    /// either.
    pub(super) fn worktree(&self, dir: &Path) -> Worktree {
        if let Some(known) = self.worktrees.borrow().get(dir) {
            return known.clone();
        }
        // Whether git ran and answered; a skipped or timed-out probe is unknown.
        let mut answered = false;
        let top = self.budget.probe(
            || format!("git worktree of {}", dir.display()),
            |timeout| {
                let probe = self.sys.git_toplevel(dir, timeout);
                answered = !probe.timed_out();
                probe
            },
        );
        let known = match top {
            Some(top) => canonical_dir(&top).map_or(Worktree::Unknown, Worktree::Git),
            None if answered => Worktree::Plain,
            None => Worktree::Unknown,
        };
        self.worktrees
            .borrow_mut()
            .insert(dir.to_path_buf(), known.clone());
        known
    }

    /// A session (or dev server) directory is related to a root when one
    /// contains the other and both are in the same worktree. Different
    /// worktrees of one repository are never related; two directories outside
    /// any worktree are related by containment alone. Unknown is unrelated.
    pub(super) fn related(&self, cwd: &Path, root: &Path) -> bool {
        let Some(cwd) = canonical_dir(cwd) else {
            return false;
        };
        if !(cwd.starts_with(root) || root.starts_with(&cwd)) {
            return false;
        }
        match (self.worktree(&cwd), self.worktree(root)) {
            (Worktree::Git(a), Worktree::Git(b)) => a == b,
            (Worktree::Plain, Worktree::Plain) => true,
            _ => false,
        }
    }

    /// The canonical working directory of the process listening on a
    /// loopback port.
    pub(super) fn port_owner(&self, port: u16) -> Option<PathBuf> {
        if let Some(known) = self.owners.borrow().get(&port) {
            return known.clone();
        }
        let owner = self
            .budget
            .probe(
                || format!("owner of port {port}"),
                |timeout| self.sys.port_cwd(port, timeout),
            )
            .and_then(|cwd| canonical_dir(&cwd));
        self.owners.borrow_mut().insert(port, owner.clone());
        owner
    }

    /// Tracked `.chrome-use/inspect.json` files of a worktree, relative to it.
    pub(super) fn configs(&self, top: &Path) -> Vec<String> {
        self.budget
            .probe(
                || format!("project configs in {}", top.display()),
                |timeout| self.sys.git_configs(top, timeout),
            )
            .map(|out| super::config::listed(&out))
            .unwrap_or_default()
    }

    pub(super) fn relation(&self, top: &Path, rev: &str) -> Relation {
        if let Some(known) = self.relations.borrow().get(top) {
            return *known;
        }
        let relation = self
            .budget
            .probe(
                || format!("revision {rev} in {}", top.display()),
                |timeout| self.sys.git_revision(top, rev, timeout),
            )
            .unwrap_or(Relation::Unknown);
        self.relations
            .borrow_mut()
            .insert(top.to_path_buf(), relation);
        relation
    }

    pub(super) fn skipped(&self) -> Vec<String> {
        self.budget.skipped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_relation_is_parsed_from_rev_list_counts() {
        assert_eq!(Relation::from_counts("0\t0\n"), Relation::Same);
        assert_eq!(Relation::from_counts("0\t3\n"), Relation::Ahead(3));
        assert_eq!(Relation::from_counts("2\t0"), Relation::Behind(2));
        assert_eq!(
            Relation::from_counts("2\t5"),
            Relation::Diverged {
                ahead: 5,
                behind: 2
            }
        );
        for bad in ["", "x\ty", "1", "1\t2\t3", "-1\t0"] {
            assert_eq!(Relation::from_counts(bad), Relation::Unknown, "{bad:?}");
        }
        assert!(Relation::Same.rank() < Relation::Ahead(1).rank());
        assert!(Relation::Ahead(9).rank() < Relation::Behind(1).rank());
        assert_eq!(Relation::Ahead(3).view()["relation"], "ahead");
        assert_eq!(revision("abc1234"), Some("abc1234"));
        assert_eq!(revision(&"a".repeat(40)), Some(&*"a".repeat(40)));
        for bad in [
            "abc123",
            "ABC1234",
            "abc1234 ",
            "--all",
            &"a".repeat(41),
            "abc123g",
        ] {
            assert_eq!(revision(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_budget_skips_calls_once_spent_and_charges_timeouts_in_full() {
        let budget = Budget::new(TOTAL);
        let slices: RefCell<Vec<Duration>> = RefCell::default();
        for i in 0..5 {
            let got = budget.probe(
                || format!("call {i}"),
                |slice| {
                    slices.borrow_mut().push(slice);
                    Probe::<()>::TimedOut
                },
            );
            assert!(got.is_none());
        }
        // 3 s + 3 s + the 2 s left; the rest never ran.
        assert_eq!(
            *slices.borrow(),
            [PER_CALL, PER_CALL, Duration::from_secs(2)]
        );
        let skipped = budget.skipped();
        assert_eq!(skipped.len(), 5);
        assert!(skipped[0].contains("timed out"));
        assert!(skipped[4].contains("budget spent"));
        let fresh = Budget::new(TOTAL);
        assert_eq!(fresh.probe(|| "x".into(), |_| Probe::Found(7)), Some(7));
        assert_eq!(fresh.probe(|| "y".into(), |_| Probe::<u8>::Absent), None);
        assert!(fresh.skipped().is_empty());
    }
}
