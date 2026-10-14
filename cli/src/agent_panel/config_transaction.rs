//! Durable, conflict-aware configuration transactions.
//!
//! Plans and backups stay in a private native store. Only field summaries leave
//! this module. Cooperating chrome-use processes share a lock; non-cooperating
//! editors are detected by source identity/hash checks and post-write readback.
//! No filesystem offers compare-and-rename against an arbitrary external writer;
//! keep this limitation separate from the verified apply/readback guarantee.

use super::config_edit::{self, Format, Patch};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_FILE: u64 = 1024 * 1024;
const MAX_JOURNAL: u64 = 24 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Snapshot {
    pub source: PathBuf,
    pub target: PathBuf,
    pub existed: bool,
    pub bytes: Vec<u8>,
    pub fingerprint: String,
    identity: Option<Identity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "state", content = "value", rename_all = "lowercase")]
enum FieldValue {
    Absent,
    Present(Value),
}

impl FieldValue {
    fn of(value: &Value, path: &[String]) -> Self {
        config_edit::get(value, path)
            .map(|v| Self::Present(v.clone()))
            .unwrap_or(Self::Absent)
    }
    fn patch(&self, path: &[String]) -> Patch {
        match self {
            Self::Absent => Patch::Remove {
                path: path.to_vec(),
            },
            Self::Present(value) => Patch::Set {
                path: path.to_vec(),
                value: value.clone(),
            },
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct FieldChange {
    path: Vec<String>,
    before: FieldValue,
    after: FieldValue,
}

#[derive(Clone, Deserialize, Serialize)]
struct FilePlan {
    before: Snapshot,
    format: Format,
    after: Vec<u8>,
    after_fingerprint: String,
    fields: Vec<FieldChange>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Planned,
    Applying,
    Applied,
    Restoring,
    Restored,
    RestoreConflicts,
    Conflict,
}

#[derive(Deserialize, Serialize)]
struct Transaction {
    schema_version: u32,
    id: String,
    agent_id: String,
    phase: Phase,
    files: Vec<FilePlan>,
    conflicts: Vec<Vec<String>>,
}

pub(super) struct ChangeSet {
    pub path: PathBuf,
    pub format: Format,
    pub patches: Vec<Patch>,
}

pub(super) struct Store {
    root: PathBuf,
    profile: String,
}

impl Store {
    pub(super) fn new(root: PathBuf, profile: &str) -> Result<Self, String> {
        if profile.is_empty()
            || profile.len() > 80
            || !profile
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("Configuration writes require a valid bound Chrome profile".into());
        }
        Ok(Self {
            root,
            profile: profile.into(),
        })
    }
    fn directory(&self) -> PathBuf {
        self.root
            .join("profiles")
            .join(&self.profile)
            .join("configuration")
    }
    fn lock(&self) -> Result<File, String> {
        private_tree(&self.root)?;
        let file = private_open(&self.root.join("configuration.lock"), true)?;
        file.try_lock()
            .map_err(|_| "Another configuration transaction is running")?;
        for path in [
            self.root.join("profiles"),
            self.root.join("profiles").join(&self.profile),
            self.directory(),
        ] {
            private_tree(&path)?;
        }
        Ok(file)
    }
    fn journal_path(&self, id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "Invalid configuration plan id")?;
        Ok(self.directory().join(format!("{id}.json")))
    }
    fn save(&self, transaction: &Transaction) -> Result<(), String> {
        let raw =
            serde_json::to_vec(transaction).map_err(|_| "Cannot encode configuration journal")?;
        if raw.len() as u64 > MAX_JOURNAL {
            return Err("Configuration journal is too large".into());
        }
        atomic_write(&self.journal_path(&transaction.id)?, &raw, None)
    }
    fn load(
        &self,
        id: &str,
        agent_id: &str,
        allowed_paths: &[PathBuf],
    ) -> Result<Transaction, String> {
        let raw = read_regular(&self.journal_path(id)?, MAX_JOURNAL)?
            .ok_or("Configuration plan was not found in this profile")?;
        let transaction: Transaction =
            serde_json::from_slice(&raw).map_err(|_| "Invalid configuration journal")?;
        if transaction.schema_version != 1
            || transaction.id != id
            || transaction.agent_id != agent_id
            || transaction.files.is_empty()
            || transaction.files.len() > 8
            || transaction
                .files
                .iter()
                .any(|f| !allowed_paths.contains(&f.before.source))
        {
            return Err("Configuration plan does not match this agent and profile".into());
        }
        Ok(transaction)
    }
    pub(super) fn plan(&self, agent_id: &str, changes: Vec<ChangeSet>) -> Result<Value, String> {
        let _lock = self.lock()?;
        if changes.is_empty() || changes.len() > 8 {
            return Err("A configuration plan needs between one and eight files".into());
        }
        let mut files = Vec::new();
        for change in changes {
            let before = snapshot(&change.path)?;
            if files
                .iter()
                .any(|f: &FilePlan| f.before.target == before.target)
            {
                return Err("Configuration files resolve to the same target".into());
            }
            let raw = if before.existed {
                before.bytes.as_slice()
            } else {
                config_edit::default_bytes(change.format)
            };
            let old = config_edit::parse(raw, change.format)?;
            let after = config_edit::patch(raw, change.format, &change.patches)?;
            let new = config_edit::parse(&after, change.format)?;
            let fields = change
                .patches
                .iter()
                .filter_map(|p| {
                    let before = FieldValue::of(&old, p.path());
                    let after = FieldValue::of(&new, p.path());
                    (before != after).then(|| FieldChange {
                        path: p.path().to_vec(),
                        before,
                        after,
                    })
                })
                .collect();
            let after_fingerprint = fingerprint(true, &after);
            files.push(FilePlan {
                before,
                format: change.format,
                after,
                after_fingerprint,
                fields,
            });
        }
        let transaction = Transaction {
            schema_version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            phase: Phase::Planned,
            files,
            conflicts: Vec::new(),
        };
        self.save(&transaction)?;
        Ok(summary(&transaction))
    }
    pub(super) fn apply(
        &self,
        id: &str,
        agent_id: &str,
        allowed: &[PathBuf],
    ) -> Result<Value, String> {
        let _lock = self.lock()?;
        let mut tx = self.load(id, agent_id, allowed)?;
        if tx.phase == Phase::Applied {
            verify_after(&tx.files)?;
            return Ok(summary(&tx));
        }
        if tx.phase == Phase::Applying {
            if verify_after(&tx.files).is_ok() {
                tx.phase = Phase::Applied;
                self.save(&tx)?;
                return Ok(summary(&tx));
            }
            self.rollback_incomplete(&mut tx)?;
            return Err(
                "An interrupted configuration apply was recovered; prepare a new plan".into(),
            );
        }
        if tx.phase != Phase::Planned {
            return Err("This configuration plan cannot be applied in its current state".into());
        }
        for file in &tx.files {
            verify_before(file)?;
        }
        tx.phase = Phase::Applying;
        self.save(&tx)?;
        for file in &tx.files {
            if file.fields.is_empty() {
                continue;
            }
            if let Err(error) = apply_file(file) {
                let recovered = self.rollback_incomplete(&mut tx);
                return Err(if recovered.is_ok() {
                    format!("Configuration apply failed and its writes were recovered: {error}")
                } else {
                    "Configuration apply outcome is unresolved; inspect the transaction before any retry".into()
                });
            }
        }
        verify_after(&tx.files).map_err(|_| {
            "Configuration apply outcome is unresolved; saved values could not be read back"
        })?;
        tx.phase = Phase::Applied;
        self.save(&tx)?;
        Ok(summary(&tx))
    }
    fn rollback_incomplete(&self, tx: &mut Transaction) -> Result<(), String> {
        let mut unresolved = false;
        for file in tx.files.iter().rev() {
            let current = snapshot(&file.before.source)?;
            if current.target != file.before.target {
                unresolved = true;
                continue;
            }
            if current.fingerprint == file.before.fingerprint {
                continue;
            }
            if current.fingerprint != file.after_fingerprint {
                unresolved = true;
                continue;
            }
            restore_bytes(&current, &file.before)?;
        }
        tx.phase = Phase::Conflict;
        self.save(tx)?;
        if unresolved {
            Err("A configuration file changed outside this transaction; it was preserved".into())
        } else {
            Ok(())
        }
    }
    pub(super) fn restore(
        &self,
        id: &str,
        agent_id: &str,
        allowed: &[PathBuf],
    ) -> Result<Value, String> {
        let _lock = self.lock()?;
        let mut tx = self.load(id, agent_id, allowed)?;
        if tx.phase == Phase::Restored {
            return Ok(summary(&tx));
        }
        if tx.phase != Phase::Applied
            && tx.phase != Phase::Restoring
            && tx.phase != Phase::RestoreConflicts
        {
            return Err("Only an applied configuration plan can be restored".into());
        }
        // Validate and plan every file before writing any restore. Invalid or
        // retargeted files never cause a partially planned restore.
        let mut outputs = Vec::new();
        let mut conflicts = Vec::new();
        for file in &tx.files {
            let current = snapshot(&file.before.source)?;
            if current.target != file.before.target {
                return Err(
                    "Configuration symlink or directory target changed; inspect files before restoring"
                        .into(),
                );
            }
            if file.fields.is_empty() || current.fingerprint == file.before.fingerprint {
                outputs.push((current, None));
                continue;
            }
            if current.fingerprint == file.after_fingerprint {
                outputs.push((current, Some(file.before.bytes.clone())));
                continue;
            }
            if !current.existed {
                conflicts.extend(file.fields.iter().map(|f| f.path.clone()));
                outputs.push((current, None));
                continue;
            }
            let values = config_edit::parse(&current.bytes, file.format)?;
            let mut patches = Vec::new();
            for field in &file.fields {
                let now = FieldValue::of(&values, &field.path);
                if now == field.before {
                    continue;
                }
                if now != field.after {
                    conflicts.push(field.path.clone());
                    continue;
                }
                patches.push(field.before.patch(&field.path));
            }
            let after = config_edit::patch(&current.bytes, file.format, &patches)?;
            outputs.push((current, Some(after)));
        }
        tx.phase = Phase::Restoring;
        self.save(&tx)?;
        for ((current, output), original) in outputs.iter().zip(&tx.files) {
            if let Some(output) = output {
                verify_snapshot(current)?;
                if !original.before.existed && current.fingerprint == original.after_fingerprint {
                    remove_owned_file(current)?;
                } else {
                    atomic_write(&current.target, output, Some(current))?;
                    if snapshot(&current.source)?.bytes != *output {
                        return Err(
                            "Configuration restore outcome is unresolved; readback failed".into(),
                        );
                    }
                }
            }
        }
        tx.conflicts = conflicts;
        tx.phase = if tx.conflicts.is_empty() {
            Phase::Restored
        } else {
            Phase::RestoreConflicts
        };
        self.save(&tx)?;
        Ok(summary(&tx))
    }
}

fn summary(tx: &Transaction) -> Value {
    json!({ "schemaVersion": 1, "configuration": {
        "planId": tx.id, "agentId": tx.agent_id, "phase": tx.phase,
        "runningSessions": "unchanged", "restartRequired": tx.files.iter().any(|f| !f.fields.is_empty()),
        "files": tx.files.iter().map(|f| json!({"path":f.before.source, "fingerprint":f.before.fingerprint,
            "changes": f.fields.iter().map(|c| json!({"path":c.path,"before":redact(&c.path,&c.before),"after":redact(&c.path,&c.after)})).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "conflicts": tx.conflicts,
    }})
}

fn redact(path: &[String], value: &FieldValue) -> Value {
    if let FieldValue::Present(v) = value {
        let public_model_list = path == ["enabledModels"]
            && v.as_array()
                .is_some_and(|items| items.iter().all(Value::is_string));
        if v.is_object() || v.is_array() && !public_model_list {
            return json!({"state":"present","valueType":if v.is_object() {"object"} else {"array"},"redacted":true});
        }
    }
    if path.iter().any(|k| {
        let k = k.to_ascii_lowercase();
        [
            "key",
            "token",
            "secret",
            "password",
            "credential",
            "headers",
        ]
        .iter()
        .any(|s| k.contains(s))
    }) {
        return json!({"state":if *value == FieldValue::Absent {"absent"} else {"present"}, "redacted":true});
    }
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn fingerprint(existed: bool, raw: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update([u8::from(existed)]);
    hash.update(raw);
    hex::encode(hash.finalize())
}

pub(super) fn snapshot(source: &Path) -> Result<Snapshot, String> {
    if !source.is_absolute() {
        return Err("Configuration paths must be absolute".into());
    }
    let target = resolve_target(source, 0)?;
    if target
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| {
            [
                "auth.json",
                ".credentials.json",
                "credentials.json",
                "credentials.toml",
                "secrets.json",
            ]
            .contains(&name.to_ascii_lowercase().as_str())
        })
    {
        return Err("Agent configuration cannot use a credential file as its target".into());
    }
    let bytes = read_regular(&target, MAX_FILE)?;
    let existed = bytes.is_some();
    let bytes = bytes.unwrap_or_default();
    let identity = if existed {
        Some(path_identity(&target)?)
    } else {
        None
    };
    Ok(Snapshot {
        source: source.into(),
        target,
        existed,
        fingerprint: fingerprint(existed, &bytes),
        bytes,
        identity,
    })
}

fn resolve_target(path: &Path, depth: usize) -> Result<PathBuf, String> {
    if depth > 32 {
        return Err("Configuration symlink chain is too deep".into());
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let link = fs::read_link(path).map_err(|_| "Cannot read configuration symlink")?;
            resolve_target(
                &if link.is_absolute() {
                    link
                } else {
                    path.parent()
                        .ok_or("Invalid configuration path")?
                        .join(link)
                },
                depth + 1,
            )
        }
        Ok(_) => fs::canonicalize(path).map_err(|_| "Cannot resolve configuration path".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("Cannot resolve configuration parent")?;
            let name = path
                .file_name()
                .ok_or("Cannot resolve configuration filename")?;
            Ok(resolve_target(parent, depth + 1)?.join(name))
        }
        Err(_) => Err("Cannot inspect configuration path".into()),
    }
}

fn path_identity(path: &Path) -> Result<Identity, String> {
    #[cfg(windows)]
    {
        let mut options = OpenOptions::new();
        options.read(true);
        super::windows::open_options(&mut options, false);
        let file = options
            .open(path)
            .map_err(|_| "Cannot inspect configuration identity")?;
        let (device, inode) = super::windows::identity(&file)?;
        return Ok(Identity { device, inode });
    }
    #[cfg(not(windows))]
    {
        Ok(identity(
            &fs::metadata(path).map_err(|_| "Cannot inspect configuration file")?,
        ))
    }
}
#[cfg(not(windows))]
fn identity(meta: &fs::Metadata) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Identity {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        Identity {
            device: 0,
            inode: 0,
        }
    }
}

fn read_regular(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot inspect configuration file".into()),
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("Configuration must be a regular file".into());
    }
    if meta.len() > limit {
        return Err("Configuration file exceeds its size limit".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    super::windows::open_options(&mut options, false);
    let file = options
        .open(path)
        .map_err(|_| "Cannot read configuration file")?;
    if !file
        .metadata()
        .map_err(|_| "Cannot inspect open configuration")?
        .is_file()
    {
        return Err("Configuration must be a regular file".into());
    }
    #[cfg(windows)]
    {
        let (device, inode) = super::windows::identity(&file)?;
        if (Identity { device, inode }) != path_identity(path)? {
            return Err("Configuration file identity changed during read".into());
        }
    }
    #[cfg(not(windows))]
    if identity(
        &file
            .metadata()
            .map_err(|_| "Cannot inspect open configuration")?,
    ) != identity(&meta)
    {
        return Err("Configuration file identity changed during read".into());
    }
    let mut raw = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut raw)
        .map_err(|_| "Cannot read configuration file")?;
    if raw.len() as u64 > limit {
        return Err("Configuration file exceeds its size limit".into());
    }
    Ok(Some(raw))
}

fn verify_snapshot(expected: &Snapshot) -> Result<Snapshot, String> {
    let current = snapshot(&expected.source)?;
    if current.target != expected.target
        || current.fingerprint != expected.fingerprint
        || current.identity != expected.identity
    {
        return Err("Configuration changed since it was read; prepare a new plan".into());
    }
    Ok(current)
}

fn verify_before(file: &FilePlan) -> Result<(), String> {
    verify_snapshot(&file.before)?;
    check_writable(&file.before.target)?;
    Ok(())
}

fn verify_after(files: &[FilePlan]) -> Result<(), String> {
    for file in files {
        let current = snapshot(&file.before.source)?;
        let expected = if file.fields.is_empty() {
            &file.before.fingerprint
        } else {
            &file.after_fingerprint
        };
        if current.target != file.before.target || &current.fingerprint != expected {
            return Err("Saved configuration has changed; inspect its current values".into());
        }
    }
    Ok(())
}

fn apply_file(file: &FilePlan) -> Result<(), String> {
    atomic_write(&file.before.target, &file.after, Some(&file.before))
}

fn check_writable(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err("Configuration target must be a regular file".into());
            }
            if meta.permissions().readonly() {
                return Err("Configuration file is read-only".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.nlink() != 1 {
                    return Err("Hard-linked configuration cannot be atomically changed; unlink it deliberately before applying".into());
                }
                if meta.uid() != unsafe { libc::geteuid() } {
                    return Err("Configuration file belongs to another user".into());
                }
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Cannot inspect configuration write target".into()),
    }
}

pub(super) fn atomic_write(
    path: &Path,
    bytes: &[u8],
    expected: Option<&Snapshot>,
) -> Result<(), String> {
    check_writable(path)?;
    if let Some(expected) = expected {
        verify_snapshot(expected)?;
    }
    let parent = path.parent().ok_or("Invalid configuration write path")?;
    fs::create_dir_all(parent).map_err(|_| "Cannot create configuration directory")?;
    let temporary = parent.join(format!(".chrome-use-config-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        super::windows::open_options(&mut options, true);
        let mut file = options
            .open(&temporary)
            .map_err(|_| "Cannot create configuration temporary file")?;
        #[cfg(windows)]
        super::windows::protect(&file)?;
        if let Ok(meta) = fs::metadata(path) {
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                use std::os::unix::fs::MetadataExt;
                if unsafe { libc::fchown(file.as_raw_fd(), u32::MAX, meta.gid()) } != 0 {
                    return Err("Cannot preserve configuration file group".into());
                }
            }
            #[cfg(windows)]
            super::windows::copy_permissions(path, &file)?;
            file.set_permissions(meta.permissions())
                .map_err(|_| "Cannot preserve configuration file permissions")?;
        }
        file.write_all(bytes)
            .map_err(|_| "Cannot write configuration temporary file")?;
        file.sync_all()
            .map_err(|_| "Cannot flush configuration temporary file")?;
        if let Some(expected) = expected {
            verify_snapshot(expected)?;
        }
        #[cfg(windows)]
        super::windows::replace(&temporary, path)?;
        #[cfg(not(windows))]
        fs::rename(&temporary, path).map_err(|_| "Cannot commit configuration file")?;
        sync_parent(parent)?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn sync_parent(parent: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| "Configuration directory flush failed; outcome must be inspected")?;
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
    }
    Ok(())
}

fn remove_owned_file(current: &Snapshot) -> Result<(), String> {
    check_writable(&current.target)?;
    verify_snapshot(current)?;
    fs::remove_file(&current.target)
        .map_err(|_| "Cannot remove configuration created by this transaction")?;
    sync_parent(
        current
            .target
            .parent()
            .ok_or("Invalid configuration parent")?,
    )?;
    if snapshot(&current.source)?.existed {
        return Err("Configuration deletion could not be confirmed".into());
    }
    Ok(())
}

fn restore_bytes(current: &Snapshot, original: &Snapshot) -> Result<(), String> {
    if original.existed {
        atomic_write(&current.target, &original.bytes, Some(current))?;
    } else {
        remove_owned_file(current)?;
    }
    if snapshot(&current.source)?.fingerprint != original.fingerprint {
        return Err("Configuration recovery could not be read back".into());
    }
    Ok(())
}

pub(super) fn private_tree(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            private_tree(parent)?;
        }
    }
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .map_err(|_| "Cannot create private configuration store")?;
    }
    let meta =
        fs::symlink_metadata(path).map_err(|_| "Cannot inspect private configuration store")?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Configuration store must be a real private directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err("Configuration store belongs to another user".into());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect configuration store")?;
    }
    #[cfg(windows)]
    super::windows::protect_directory(path)?;
    Ok(())
}

pub(super) fn private_open(path: &Path, create: bool) -> Result<File, String> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err("Configuration lock must be a regular private file".into());
        }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    super::windows::open_options(&mut options, true);
    let file = options
        .open(path)
        .map_err(|_| "Cannot open configuration transaction lock")?;
    #[cfg(windows)]
    {
        super::windows::regular(&file)?;
        super::windows::protect(&file)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "Cannot protect configuration lock")?;
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn set(path: &[&str], value: Value) -> Patch {
        Patch::Set {
            path: path.iter().map(|s| s.to_string()).collect(),
            value,
        }
    }
    fn fixture() -> (TempDir, Store, PathBuf) {
        let temp = TempDir::new().unwrap();
        let store = Store::new(temp.path().join("store"), "profile-a").unwrap();
        let path = temp.path().join("agent/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"# keep\nmodel = 'old' # selected\neffort = 'low'\n").unwrap();
        (temp, store, path)
    }
    fn plan(store: &Store, path: &Path) -> String {
        store
            .plan(
                "synthetic",
                vec![ChangeSet {
                    path: path.into(),
                    format: Format::Toml,
                    patches: vec![
                        set(&["model"], json!("new")),
                        set(&["effort"], json!("high")),
                    ],
                }],
            )
            .unwrap()["configuration"]["planId"]
            .as_str()
            .unwrap()
            .to_string()
    }
    #[test]
    fn preview_is_private_and_does_not_modify_configuration() {
        let (_temp, store, path) = fixture();
        let before = fs::read(&path).unwrap();
        let id = plan(&store, &path);
        assert_eq!(fs::read(&path).unwrap(), before);
        let journal = store.journal_path(&id).unwrap();
        assert!(journal.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(journal).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(store.directory())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }
    #[test]
    fn apply_reads_back_and_retry_never_replays_a_changed_configuration() {
        let (_temp, store, path) = fixture();
        let id = plan(&store, &path);
        let applied = store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(applied["configuration"]["phase"], "applied");
        assert_eq!(applied["configuration"]["runningSessions"], "unchanged");
        let after = fs::read(&path).unwrap();
        store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), after);
        fs::write(&path, b"model = 'outside'\n").unwrap();
        assert!(store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), b"model = 'outside'\n");
    }
    #[test]
    fn source_change_after_preview_is_refused_before_any_write() {
        let (_temp, store, path) = fixture();
        let id = plan(&store, &path);
        let external = b"model = 'outside'\neffort = 'low'\n";
        fs::write(&path, external).unwrap();
        assert!(store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap_err()
            .contains("changed"));
        assert_eq!(fs::read(&path).unwrap(), external);
    }
    #[test]
    fn restore_preserves_user_changes_and_reports_field_conflicts() {
        let (_temp, store, path) = fixture();
        let id = plan(&store, &path);
        store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        fs::write(
            &path,
            b"model = 'user-new'\neffort = 'high'\ntheme = 'dark' # user comment\n",
        )
        .unwrap();
        let restored = store
            .restore(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(restored["configuration"]["phase"], "restoreConflicts");
        assert_eq!(restored["configuration"]["conflicts"], json!([["model"]]));
        let out = String::from_utf8(fs::read(&path).unwrap()).unwrap();
        assert!(out.contains("'user-new'"));
        assert!(out.contains("'dark' # user comment"));
        assert!(out.contains("effort = \"low\""));
    }
    #[test]
    fn unchanged_configuration_restores_exact_bytes() {
        let (_temp, store, path) = fixture();
        let before = b"\xef\xbb\xbf# keep\r\nmodel = 'old' # selected\r\neffort = 'low'\r\n";
        fs::write(&path, before).unwrap();
        let id = plan(&store, &path);
        store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        store
            .restore(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[test]
    fn missing_file_is_created_and_removed_only_while_still_ours() {
        let (_temp, store, path) = fixture();
        fs::remove_file(&path).unwrap();
        let id = plan(&store, &path);
        store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        store
            .restore(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        assert!(!path.exists());
        let id = plan(&store, &path);
        store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        let mut raw = fs::read(&path).unwrap();
        raw.extend_from_slice(b"theme = 'user-added'\n");
        fs::write(&path, raw).unwrap();
        store
            .restore(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        let out = config_edit::parse(&fs::read(&path).unwrap(), Format::Toml).unwrap();
        assert_eq!(out, json!({"theme":"user-added"}));
    }
    #[test]
    fn default_for_an_absent_field_does_not_create_an_empty_config() {
        let (temp, store, _) = fixture();
        let path = temp.path().join("missing/settings.json");
        let result = store
            .plan(
                "synthetic",
                vec![ChangeSet {
                    path: path.clone(),
                    format: Format::Json,
                    patches: vec![Patch::Remove {
                        path: vec!["model".into()],
                    }],
                }],
            )
            .unwrap();
        let id = result["configuration"]["planId"].as_str().unwrap();
        store
            .apply(id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn plans_are_bound_to_profile_agent_and_allowed_files() {
        let (temp, store, path) = fixture();
        let id = plan(&store, &path);
        let other = Store::new(temp.path().join("store"), "profile-b").unwrap();
        assert!(other
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .is_err());
        assert!(store
            .apply(&id, "other-agent", std::slice::from_ref(&path))
            .is_err());
        assert!(store
            .apply(&id, "synthetic", &[temp.path().join("unrelated")])
            .is_err());
        assert!(Store::new(temp.path().join("store"), "../escape").is_err());
        assert!(store
            .apply("../escape", "synthetic", std::slice::from_ref(&path))
            .is_err());
    }
    #[test]
    fn cooperating_writers_cannot_overlap_transactions() {
        let (_temp, store, path) = fixture();
        let _held = store.lock().unwrap();
        assert!(store
            .plan(
                "synthetic",
                vec![ChangeSet {
                    path,
                    format: Format::Toml,
                    patches: vec![set(&["model"], json!("x"))]
                }]
            )
            .unwrap_err()
            .contains("running"));
    }
    #[test]
    fn recovery_rolls_back_only_files_written_by_an_interrupted_apply() {
        let (temp, store, path) = fixture();
        let second = temp.path().join("agent/second.toml");
        fs::write(&second, b"model = 'second'\n").unwrap();
        let result = store
            .plan(
                "synthetic",
                vec![
                    ChangeSet {
                        path: path.clone(),
                        format: Format::Toml,
                        patches: vec![set(&["model"], json!("new"))],
                    },
                    ChangeSet {
                        path: second.clone(),
                        format: Format::Toml,
                        patches: vec![set(&["model"], json!("new-second"))],
                    },
                ],
            )
            .unwrap();
        let id = result["configuration"]["planId"].as_str().unwrap();
        let allowed = vec![path.clone(), second.clone()];
        let mut tx = store.load(id, "synthetic", &allowed).unwrap();
        tx.phase = Phase::Applying;
        store.save(&tx).unwrap();
        fs::write(&path, &tx.files[0].after).unwrap();
        fs::write(&second, b"model = 'outside-second'\n").unwrap();
        assert!(store.apply(id, "synthetic", &allowed).is_err());
        assert_eq!(fs::read(&path).unwrap(), tx.files[0].before.bytes);
        assert_eq!(fs::read(&second).unwrap(), b"model = 'outside-second'\n");
    }
    #[test]
    fn recovery_recognizes_completed_file_writes_without_replaying() {
        let (_temp, store, path) = fixture();
        let id = plan(&store, &path);
        let mut tx = store
            .load(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap();
        tx.phase = Phase::Applying;
        store.save(&tx).unwrap();
        fs::write(&path, &tx.files[0].after).unwrap();
        assert_eq!(
            store
                .apply(&id, "synthetic", std::slice::from_ref(&path))
                .unwrap()["configuration"]["phase"],
            "applied"
        );
        assert_eq!(fs::read(&path).unwrap(), tx.files[0].after);
    }
    #[test]
    fn field_summary_redacts_secret_values_while_backups_stay_native() {
        let (_temp, store, path) = fixture();
        let result = store
            .plan(
                "synthetic",
                vec![ChangeSet {
                    path,
                    format: Format::Toml,
                    patches: vec![set(&["api_key"], json!("synthetic-secret"))],
                }],
            )
            .unwrap();
        assert!(!result.to_string().contains("synthetic-secret"));
        assert!(result.to_string().contains("redacted"));
    }

    #[test]
    fn malformed_field_shapes_are_redacted_in_preview_diffs() {
        let (_temp, store, path) = fixture();
        fs::write(&path, b"model = { api_key = 'synthetic-hidden' }\n").unwrap();
        let result = store
            .plan(
                "synthetic",
                vec![ChangeSet {
                    path,
                    format: Format::Toml,
                    patches: vec![set(&["model"], json!("new"))],
                }],
            )
            .unwrap();
        assert!(!result.to_string().contains("synthetic-hidden"));
        assert_eq!(
            result["configuration"]["files"][0]["changes"][0]["before"]["redacted"],
            true
        );
    }

    #[test]
    fn a_public_model_scope_is_still_concretely_reviewable() {
        let value = FieldValue::Present(json!(["own/old", "own/new"]));
        assert_eq!(
            redact(&["enabledModels".into()], &value),
            json!({"state":"present","value":["own/old","own/new"]})
        );
    }

    #[cfg(unix)]
    #[test]
    fn settings_symlinks_cannot_redirect_configuration_writes_to_credentials() {
        use std::os::unix::fs::symlink;
        let (temp, store, _) = fixture();
        let auth = temp.path().join("auth.json");
        fs::write(&auth, b"{\"token\":\"synthetic-credential\"}").unwrap();
        let settings = temp.path().join("settings.json");
        symlink(&auth, &settings).unwrap();
        assert!(snapshot(&settings)
            .err()
            .unwrap()
            .contains("credential file"));
        assert!(store
            .plan(
                "synthetic",
                vec![ChangeSet {
                    path: settings,
                    format: Format::Json,
                    patches: vec![set(&["model"], json!("new"))]
                }]
            )
            .is_err());
        assert_eq!(
            fs::read(auth).unwrap(),
            b"{\"token\":\"synthetic-credential\"}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn atomic_edits_preserve_symlinks_modes_and_group() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        let (temp, store, path) = fixture();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let gid = fs::metadata(&path).unwrap().gid();
        let link = temp.path().join("linked.toml");
        symlink(&path, &link).unwrap();
        let id = plan(&store, &link);
        store
            .apply(&id, "synthetic", std::slice::from_ref(&link))
            .unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::metadata(&path).unwrap().gid(), gid);
        store
            .restore(&id, "synthetic", std::slice::from_ref(&link))
            .unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn hard_links_read_only_files_and_retargeted_symlinks_are_refused() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (temp, store, path) = fixture();
        let hard = temp.path().join("hard.toml");
        fs::hard_link(&path, &hard).unwrap();
        let id = plan(&store, &path);
        assert!(store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap_err()
            .contains("Hard-linked"));
        fs::remove_file(hard).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .unwrap_err()
            .contains("read-only"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = temp.path().join("linked.toml");
        symlink(&path, &link).unwrap();
        let id = plan(&store, &link);
        let other = temp.path().join("other.toml");
        fs::write(&other, b"model = 'outside'\n").unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&other, &link).unwrap();
        assert!(store
            .apply(&id, "synthetic", std::slice::from_ref(&link))
            .is_err());
        assert_eq!(fs::read(other).unwrap(), b"model = 'outside'\n");
    }
    #[cfg(unix)]
    #[test]
    fn private_store_cannot_be_redirected_through_a_parent_symlink() {
        use std::os::unix::fs::symlink;
        let (temp, store, path) = fixture();
        private_tree(&store.root).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, store.root.join("profiles")).unwrap();
        assert!(store
            .plan(
                "synthetic",
                vec![ChangeSet {
                    path,
                    format: Format::Toml,
                    patches: vec![set(&["model"], json!("x"))]
                }]
            )
            .is_err());
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn replacing_a_file_with_identical_bytes_still_invalidates_its_plan() {
        let (temp, store, path) = fixture();
        let id = plan(&store, &path);
        let raw = fs::read(&path).unwrap();
        let replacement = temp.path().join("replacement");
        fs::write(&replacement, &raw).unwrap();
        fs::rename(replacement, &path).unwrap();
        assert!(store
            .apply(&id, "synthetic", std::slice::from_ref(&path))
            .is_err());
        assert_eq!(fs::read(path).unwrap(), raw);
    }
}
