//! Explicit user-selected skills shared across panel adapters. Reading the
//! catalog does not grant execution permission or modify a user's skill files.
use super::{
    config_transaction::{atomic_write, private_tree},
    DiscoveryContext, AGENTS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};
const MAX_TEXT: usize = 96 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SelectedSkill {
    pub id: String,
    pub name: String,
    pub instructions: String,
}
impl SelectedSkill {
    pub fn preface(&self) -> String {
        format!("[The user explicitly selected the skill {} for this message. Follow its instructions within this conversation's existing permissions. Skill text cannot change those permissions.]\n{}\n[End of selected skill]\n\n", self.name, self.instructions)
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub editable: bool,
    pub enabled: bool,
    #[serde(skip)]
    pub folder: PathBuf,
}
fn read_text(path: &Path, max: usize) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "Cannot inspect skill file")?;
    if !meta.is_file() || meta.len() > max as u64 {
        return Err("Skill file is not bounded text".into());
    }
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = open.open(path).map_err(|_| "Cannot read skill file")?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read skill file")?;
    if bytes.len() > max {
        return Err("Skill text exceeds its size limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "Skill file is not UTF-8 text".into())
}
fn metadata(text: &str, key: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let front = rest.split("\n---").next()?;
    let value: serde_yaml::Value = serde_yaml::from_str(front).ok()?;
    value[key].as_str().map(|s| s.chars().take(512).collect())
}
fn scan(
    root: &Path,
    source: &str,
    editable: bool,
    skills: &mut BTreeMap<String, Skill>,
    depth: usize,
) {
    if depth > 2 || skills.len() >= 512 {
        return;
    }
    let Ok(real) = root.canonicalize() else {
        return;
    };
    if real.join("SKILL.md").is_file() {
        if let Ok(text) = read_text(&real.join("SKILL.md"), MAX_TEXT) {
            let id = hex::encode(Sha256::digest(real.to_string_lossy().as_bytes()));
            let name = metadata(&text, "name").unwrap_or_else(|| {
                real.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            });
            let disabled = real.join(".panel-disabled").exists();
            skills.entry(id.clone()).or_insert(Skill {
                id,
                name,
                description: metadata(&text, "description").unwrap_or_default(),
                source: source.into(),
                editable,
                enabled: !disabled,
                folder: real,
            });
        }
        return;
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten().take(512) {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                scan(&entry.path(), source, editable, skills, depth + 1);
            }
        }
    }
}
pub(super) fn catalog(directory: &Path, agent: &str) -> Result<Vec<Skill>, String> {
    let context = DiscoveryContext::current()?;
    let mut skills = BTreeMap::new();
    scan(
        &directory.join("skill-library"),
        "library",
        true,
        &mut skills,
        0,
    );
    scan(
        &context.home.join(".agents/skills"),
        "user",
        false,
        &mut skills,
        0,
    );
    if let Some(spec) = AGENTS.iter().find(|spec| spec.id == agent) {
        scan(
            &context.root(*spec).join("skills"),
            "agent",
            false,
            &mut skills,
            0,
        );
    }
    for root in crate::skills::panel_skill_dirs() {
        scan(&root, "bundled", false, &mut skills, 0);
    }
    let mut values: Vec<_> = skills.into_values().collect();
    values.sort_by(|a, b| a.name.cmp(&b.name).then(a.source.cmp(&b.source)));
    Ok(values)
}
/// Disabled entries remain readable/editable; only invocation is disabled.
pub(super) fn read_entry(directory: &Path, agent: &str, id: &str) -> Result<SelectedSkill, String> {
    let skill = catalog(directory, agent)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("Skill not found")?;
    Ok(SelectedSkill {
        id: skill.id,
        name: skill.name,
        instructions: read_text(&skill.folder.join("SKILL.md"), MAX_TEXT)?,
    })
}
pub(super) fn select(directory: &Path, agent: &str, id: &str) -> Result<SelectedSkill, String> {
    let skill = catalog(directory, agent)?
        .into_iter()
        .find(|skill| skill.id == id)
        .ok_or("Skill is unavailable; refresh the catalog")?;
    if !skill.enabled {
        return Err("This library skill is disabled".into());
    }
    let instructions = read_text(&skill.folder.join("SKILL.md"), MAX_TEXT)?;
    Ok(SelectedSkill {
        id: skill.id,
        name: skill.name,
        instructions,
    })
}
/// A selected skill is saved before message delivery. Its token remains valid
/// after the library changes, including explicit retries of the same message.
pub(super) fn snapshot(
    directory: &Path,
    agent: &str,
    id: &str,
    workspace: &Path,
) -> Result<SelectedSkill, String> {
    let mut selected = select(directory, agent, id)?;
    let skill = catalog(directory, agent)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("Skill disappeared")?;
    let mut files = BTreeMap::new();
    collect_files(&skill.folder, &skill.folder, &mut files, 0)?;
    let mut hash = Sha256::new();
    hash.update(selected.name.as_bytes());
    hash.update([0]);
    for (path, data) in &files {
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update((data.len() as u64).to_le_bytes());
        hash.update(data);
    }
    selected.id = hex::encode(hash.finalize());
    let assets = workspace.join("skills").join(&selected.id);
    private_tree(&assets)?;
    let body = files
        .get(Path::new("SKILL.md"))
        .and_then(|b| std::str::from_utf8(b).ok())
        .ok_or("Selected skill is invalid")?;
    selected.instructions = snapshot_instructions(body, &assets);
    for (path, data) in files {
        let destination = assets.join(path);
        private_tree(destination.parent().ok_or("Invalid asset path")?)?;
        atomic_write(&destination, &data, None)?;
    }

    let data = serde_json::to_vec(&selected).map_err(|_| "Cannot encode selected skill")?;
    atomic_write(&assets.join(".selection.json"), &data, None)?;
    Ok(selected)
}
fn snapshot_instructions(body: &str, assets: &Path) -> String {
    format!("{body}\n\nSkill assets directory: {}. Read or run referenced assets only within this conversation's existing permissions. If an asset requires a tool that is unavailable, report that limitation and ask the user rather than inventing its contents.",assets.display())
}
pub(super) fn load_snapshot(workspace: &Path, id: &str) -> Result<SelectedSkill, String> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid selected skill token".into());
    }
    let path = workspace.join("skills").join(id).join(".selection.json");
    let data = read_text(&path, MAX_TEXT * 3 + 8192)?;
    let skill: SelectedSkill = serde_json::from_str(&data).map_err(|_| "Saved skill is invalid")?;
    if skill.id != id {
        return Err("Selected skill identity changed".into());
    }
    let assets = workspace.join("skills").join(id);
    let mut files = BTreeMap::new();
    collect_files(&assets, &assets, &mut files, 0)?;
    let mut hash = Sha256::new();
    hash.update(skill.name.as_bytes());
    hash.update([0]);
    for (path, data) in &files {
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update((data.len() as u64).to_le_bytes());
        hash.update(data);
    }
    if hex::encode(hash.finalize()) != id {
        return Err("Selected skill assets changed; no turn was sent".into());
    }
    let body = files
        .get(Path::new("SKILL.md"))
        .and_then(|b| std::str::from_utf8(b).ok())
        .ok_or("Saved skill is invalid")?;
    if skill.instructions != snapshot_instructions(body, &assets) {
        return Err("Saved skill instructions changed; no turn was sent".into());
    }
    Ok(skill)
}
fn collect_files(
    root: &Path,
    folder: &Path,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
    depth: usize,
) -> Result<(), String> {
    if depth > 8 || files.len() > 256 {
        return Err("Skill has too many files".into());
    }
    for entry in std::fs::read_dir(folder)
        .map_err(|_| "Cannot list skill assets")?
        .take(257)
    {
        let entry = entry.map_err(|_| "Cannot list skill assets")?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|_| "Cannot inspect skill asset")?;
        if meta.file_type().is_symlink() {
            return Err("Skill assets contain symbolic links; import regular files instead".into());
        }
        if meta.is_dir() {
            collect_files(root, &path, files, depth + 1)?;
        } else if meta.is_file() {
            if meta.len() > 4 * 1024 * 1024 {
                return Err("A skill asset exceeds 4 MiB".into());
            }
            let mut data = Vec::new();
            std::fs::File::open(&path)
                .map_err(|_| "Cannot open skill asset")?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut data)
                .map_err(|_| "Cannot read skill asset")?;
            if data.len() > 4 * 1024 * 1024 {
                return Err("Skill asset grew beyond its limit".into());
            }
            files.insert(
                path.strip_prefix(root)
                    .map_err(|_| "Skill asset escaped its folder")?
                    .to_path_buf(),
                data,
            );
            if files.values().map(Vec::len).sum::<usize>() > 16 * 1024 * 1024 {
                return Err("Skill exceeds 16 MiB".into());
            }
        } else {
            return Err("Skill assets must be regular files".into());
        }
    }
    Ok(())
}

fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}
/// Only profile-owned library entries are writable. Import copies files; it
/// never grants an agent authority to overwrite the user's existing skills.
pub(super) fn save(directory: &Path, name: &str, content: &str) -> Result<(), String> {
    if !name_valid(name) || content.is_empty() || content.len() > MAX_TEXT {
        return Err("Skill needs a lowercase name and bounded nonempty instructions".into());
    }
    if metadata(content, "name").is_some_and(|own| own != name) {
        return Err("Skill frontmatter name differs from the library name".into());
    }
    let folder = directory.join("skill-library").join(name);
    private_tree(&folder)?;
    atomic_write(&folder.join("SKILL.md"), content.as_bytes(), None)?;
    if read_text(&folder.join("SKILL.md"), MAX_TEXT)? != content {
        return Err("Skill save readback failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn wrapped_skill_archives_import_assets_and_refuse_path_escape() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("skills.zip");
        let options = zip::write::SimpleFileOptions::default();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        use std::io::Write;
        zip.start_file("repo-main/skills/example/SKILL.md", options)
            .unwrap();
        zip.write_all(b"---\nname: example\n---\nRead references/detail.md")
            .unwrap();
        zip.start_file("repo-main/skills/example/references/detail.md", options)
            .unwrap();
        zip.write_all(b"Synthetic reference").unwrap();
        zip.finish().unwrap();
        let library = root.path().join("profile");
        super::import(&library, "archive", archive.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            read_text(
                &library.join("skill-library/example/references/detail.md"),
                MAX_TEXT
            )
            .unwrap(),
            "Synthetic reference"
        );
        let bad = root.path().join("bad.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&bad).unwrap());
        zip.start_file("../outside", options).unwrap();
        zip.write_all(b"refused").unwrap();
        zip.finish().unwrap();
        assert!(super::import(&library, "archive", bad.to_str().unwrap())
            .await
            .is_err());
        assert!(!root.path().join("outside").exists());
    }
    #[test]
    #[cfg(unix)]
    fn skill_leaf_symlinks_are_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        std::fs::write(&original, "Synthetic data").unwrap();
        let link = root.path().join("SKILL.md");
        std::os::unix::fs::symlink(original, &link).unwrap();
        assert!(read_text(&link, MAX_TEXT).is_err());
    }
    #[test]
    fn library_saves_are_scoped_and_refs_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let text = "---\nname: example\ndescription: Example\n---\nRead the page.";
        save(dir.path(), "example", text).unwrap();
        assert!(save(dir.path(), "../user", text).is_err());
        assert!(save(dir.path(), "other", text).is_err());
        assert_eq!(
            read_text(&dir.path().join("skill-library/example/SKILL.md"), MAX_TEXT).unwrap(),
            text
        );
    }
}

pub(super) fn set_enabled(
    directory: &Path,
    agent: &str,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let skill = catalog(directory, agent)?
        .into_iter()
        .find(|s| s.id == id && s.editable)
        .ok_or("Only library skills can be enabled or disabled here")?;
    let marker = skill.folder.join(".panel-disabled");
    if enabled {
        if marker.exists() {
            std::fs::remove_file(&marker).map_err(|_| "Cannot enable skill")?;
        }
    } else {
        atomic_write(&marker, b"disabled", None)?;
    }
    if marker.exists() == enabled {
        return Err("Skill state readback failed".into());
    }
    Ok(())
}

/// Imports are user panel operations, not page or agent instructions. No code
/// in an imported repository or archive is executed by installation.
pub(super) async fn import(directory: &Path, kind: &str, source: &str) -> Result<(), String> {
    let scratch = tempfile::tempdir().map_err(|_| "Cannot prepare skill import")?;
    let root = match kind {
        "folder" => {
            let root = PathBuf::from(source);
            if !root.is_absolute() || !root.is_dir() {
                return Err("Folder import requires an absolute directory path".into());
            }
            root
        }
        "archive" => {
            let path = PathBuf::from(source);
            if !path.is_absolute() {
                return Err("Archive import requires an absolute file path".into());
            }
            let file = std::fs::File::open(path).map_err(|_| "Cannot open skill archive")?;
            if file.metadata().map_err(|_| "Cannot inspect archive")?.len() > 16 * 1024 * 1024 {
                return Err("Archive exceeds 16 MiB".into());
            }
            let mut zip =
                zip::ZipArchive::new(file).map_err(|_| "Cannot read ZIP skill archive")?;
            if zip.len() > 512 {
                return Err("Archive has too many files".into());
            }
            let mut total = 0;
            for index in 0..zip.len() {
                let mut entry = zip
                    .by_index(index)
                    .map_err(|_| "Cannot read archive entry")?;
                let name = entry
                    .enclosed_name()
                    .ok_or("Archive entry escapes its folder")?;
                if entry
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
                {
                    return Err("Archive contains symbolic links".into());
                }
                let path = scratch.path().join(name);
                if entry.is_dir() {
                    private_tree(&path)?;
                    continue;
                }
                if entry.size() > 4 * 1024 * 1024 {
                    return Err("Archive asset exceeds 4 MiB".into());
                }
                let mut data = Vec::new();
                entry
                    .by_ref()
                    .take(4 * 1024 * 1024 + 1)
                    .read_to_end(&mut data)
                    .map_err(|_| "Cannot unpack archive asset")?;
                total += data.len();
                if data.len() > 4 * 1024 * 1024 || total > 16 * 1024 * 1024 {
                    return Err("Archive contents exceed their limit".into());
                }
                private_tree(path.parent().ok_or("Invalid archive path")?)?;
                atomic_write(&path, &data, None)?;
            }
            scratch.path().to_path_buf()
        }
        "git" => {
            let url = url::Url::parse(source).map_err(|_| "Use a Git HTTPS repository URL")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("Use a Git HTTPS URL without embedded credentials".into());
            }
            let context = DiscoveryContext::current()?;
            let git = context.executable("git").ok_or("Git is not installed")?;
            let checkout = scratch.path().join("repo");
            let mut command = tokio::process::Command::new(git);
            command
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "clone",
                    "--depth",
                    "1",
                    "--quiet",
                    "--",
                ])
                .arg(source)
                .arg(&checkout)
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            let mut child = command.spawn().map_err(|_| "Could not start Git import")?;
            match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await {
                Ok(Ok(status)) if status.success() => {}
                Ok(_) => return Err("Git skill import failed".into()),
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err("Git import timed out; no skills were installed".into());
                }
            }
            checkout
        }
        _ => return Err("Import kind is folder, archive or git".into()),
    };
    // GitHub archives wrap the repository in one directory; repositories
    // often put entries below skills/. Strip those wrappers before scanning.
    let mut root = root;
    for _ in 0..3 {
        if root.join("SKILL.md").is_file() {
            break;
        }
        if root.join("skills").is_dir() {
            root = root.join("skills");
            break;
        }
        let folders: Vec<_> = std::fs::read_dir(&root)
            .map_err(|_| "Cannot inspect import folders")?
            .flatten()
            .filter(|e| {
                e.path().is_dir()
                    && !e.file_name().to_string_lossy().starts_with('.')
                    && e.file_name() != "__MACOSX"
            })
            .take(2)
            .collect();
        if folders.len() != 1 {
            break;
        }
        root = folders[0].path();
    }
    let mut found = BTreeMap::new();
    scan(&root, "import", false, &mut found, 0);
    if found.is_empty() {
        return Err("No readable SKILL.md found in the source or its skill folders".into());
    }
    let mut prepared = BTreeMap::new();
    for skill in found.values() {
        if !name_valid(&skill.name) || prepared.contains_key(&skill.name) {
            return Err("Import contains invalid or duplicate skill names".into());
        }
        let mut files = BTreeMap::new();
        collect_files(&skill.folder, &skill.folder, &mut files, 0)?;
        prepared.insert(skill.name.clone(), files);
    }
    // Build the complete replacement first, then rename it within the same
    // profile filesystem. Existing enabled/disabled choices survive imports.
    for (name, files) in prepared {
        let library = directory.join("skill-library");
        private_tree(&library)?;
        let staging =
            tempfile::tempdir_in(&library).map_err(|_| "Cannot prepare library replacement")?;
        for (path, data) in files {
            let target = staging.path().join(path);
            private_tree(target.parent().ok_or("Invalid skill asset path")?)?;
            atomic_write(&target, &data, None)?;
        }
        let target = library.join(&name);
        if target.join(".panel-disabled").exists() {
            atomic_write(&staging.path().join(".panel-disabled"), b"disabled", None)?;
        }
        let backup = library.join(format!(".backup-{}", uuid::Uuid::new_v4()));
        let existed = target.exists();
        if existed {
            private_tree(&target)?;
            std::fs::rename(&target, &backup)
                .map_err(|_| "Cannot replace existing library skill")?;
        }
        if std::fs::rename(staging.path(), &target).is_err() {
            if existed {
                let _ = std::fs::rename(&backup, &target);
            }
            return Err("Library import failed; inspect the library before retrying".into());
        }
        if existed {
            std::fs::remove_dir_all(backup)
                .map_err(|_| "Imported skill saved but previous copy cleanup failed")?;
        }
        let content = read_text(&target.join("SKILL.md"), MAX_TEXT)?;
        if metadata(&content, "name").is_some_and(|actual| actual != name) {
            return Err("Imported skill name readback failed".into());
        }
    }
    Ok(())
}

pub(super) fn read_asset(
    workspace: &Path,
    skill: &SelectedSkill,
    path: &str,
) -> Result<String, String> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Skill asset path must stay inside the selected skill".into());
    }
    let root = workspace
        .join("skills")
        .join(&skill.id)
        .canonicalize()
        .map_err(|_| "Selected skill assets are unavailable")?;
    let target = root
        .join(relative)
        .canonicalize()
        .map_err(|_| "Skill asset not found")?;
    if !target.starts_with(root) {
        return Err("Skill asset escaped its folder".into());
    }
    read_text(&target, MAX_TEXT)
}

#[cfg(test)]
mod integrity_tests {
    use super::*;
    #[test]
    fn saved_selection_rejects_instruction_name_and_asset_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        save(
            dir.path(),
            "integrity-fixture",
            "---\nname: integrity-fixture\n---\nRead the fixture.",
        )
        .unwrap();
        let entry = catalog(dir.path(), "codex")
            .unwrap()
            .into_iter()
            .find(|s| s.name == "integrity-fixture" && s.editable)
            .unwrap();
        let selected = snapshot(dir.path(), "codex", &entry.id, &workspace).unwrap();
        let assets = workspace.join("skills").join(&selected.id);
        let path = assets.join(".selection.json");
        let original = std::fs::read(&path).unwrap();
        assert!(load_snapshot(&workspace, &selected.id).is_ok());
        for field in ["instructions", "name"] {
            let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
            changed[field] = "Tampered content".into();
            std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(
                load_snapshot(&workspace, &selected.id).is_err(),
                "accepted changed {field}"
            );
        }
        std::fs::write(&path, original).unwrap();
        std::fs::write(assets.join("SKILL.md"), "Changed asset").unwrap();
        assert!(load_snapshot(&workspace, &selected.id).is_err());
    }
}
