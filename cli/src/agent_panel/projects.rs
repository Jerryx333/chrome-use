//! The project directory a conversation's agent works in: checking a chosen
//! directory, the recent directories the panel offers, and the native folder
//! picker the service opens ("Other…").
//!
//! A project is only ever the agent's working directory. The conversation's
//! own files (MCP configuration, attachments, captures) stay in its private
//! folder, so nothing the panel writes lands in the user's project.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Directories offered by one listing.
pub(super) const MAX_RECENT: usize = 30;
/// How long the user may take to choose a folder.
pub(super) const PICK_TIMEOUT: Duration = Duration::from_secs(280);

/// System folders that are never a project: an agent working there could
/// change the computer rather than a project.
#[cfg(not(windows))]
const SYSTEM: &[&str] = &[
    "/",
    "/System",
    "/Library",
    "/Applications",
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/var",
    "/private",
    "/private/etc",
    "/private/var",
    "/private/tmp",
    "/tmp",
    "/dev",
    "/opt",
    "/Volumes",
    "/Users",
    "/home",
    "/root",
    "/boot",
    "/proc",
    "/sys",
    "/lib",
    "/lib64",
    "/snap",
];

/// Whether `dir` (canonical) is a filesystem root or a system folder.
fn system_folder(dir: &Path, home: Option<&Path>) -> bool {
    if dir.parent().is_none() {
        return true;
    }
    #[cfg(not(windows))]
    {
        if SYSTEM.iter().any(|s| dir == Path::new(s)) {
            return true;
        }
    }
    #[cfg(windows)]
    {
        let lower = dir.to_string_lossy().to_lowercase();
        let lower = lower.trim_start_matches(r"\\?\");
        let tail = lower.get(2..).unwrap_or("").trim_end_matches('\\');
        if [
            "",
            r"\windows",
            r"\program files",
            r"\program files (x86)",
            r"\programdata",
            r"\users",
        ]
        .contains(&tail)
            || tail.starts_with(r"\windows\")
        {
            return true;
        }
    }
    // The home folder itself holds every other project and the CLIs'
    // configuration; a project is a folder inside it.
    home.is_some_and(|h| h == dir)
}

/// A chosen project directory: absolute, existing, a directory, not a system
/// folder and not inside the panel's own data. Returns it canonical.
pub(super) fn validate(raw: &str, panel_root: &Path) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err("The project folder path is invalid".into());
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err("The project folder must be an absolute path".into());
    }
    let dir = path
        .canonicalize()
        .map_err(|_| format!("The project folder {raw} does not exist"))?;
    if !dir.is_dir() {
        return Err(format!("{raw} is not a folder"));
    }
    let home = dirs::home_dir().and_then(|h| h.canonicalize().ok());
    if system_folder(&dir, home.as_deref()) {
        return Err(format!(
            "{} is a system or home folder; choose a project folder inside it",
            dir.display()
        ));
    }
    let panel = panel_root
        .canonicalize()
        .unwrap_or_else(|_| panel_root.into());
    if dir.starts_with(&panel) || panel.starts_with(&dir) {
        return Err("The project folder cannot hold chrome-use's own panel data".into());
    }
    Ok(dir)
}

/// One offered directory.
#[derive(Clone, Debug)]
pub(super) struct Recent {
    pub path: PathBuf,
    /// Where it was seen: `panel`, `claude`, `codex`, `site`.
    pub sources: Vec<&'static str>,
    pub last_used: u64,
    /// Sites bound to it (Inspect & Ask / remembered projects).
    pub sites: Vec<String>,
}

/// Merge `(path, source, last used, site)` sightings into the offered list:
/// existing, allowed directories, one row each, most recent first.
pub(super) fn merge(
    seen: impl IntoIterator<Item = (PathBuf, &'static str, u64, Option<String>)>,
    panel_root: &Path,
) -> Vec<Recent> {
    let mut rows: Vec<Recent> = Vec::new();
    let mut checked = std::collections::HashMap::<PathBuf, Option<PathBuf>>::new();
    for (path, source, last_used, site) in seen {
        let canonical = checked
            .entry(path.clone())
            .or_insert_with(|| validate(&path.to_string_lossy(), panel_root).ok())
            .clone();
        let Some(path) = canonical else { continue };
        match rows.iter_mut().find(|r| r.path == path) {
            Some(row) => {
                if !row.sources.contains(&source) {
                    row.sources.push(source);
                }
                row.last_used = row.last_used.max(last_used);
                if let Some(site) = site.filter(|s| !row.sites.contains(s)) {
                    row.sites.push(site);
                }
            }
            None => rows.push(Recent {
                path,
                sources: vec![source],
                last_used,
                sites: site.into_iter().collect(),
            }),
        }
    }
    rows.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    rows.truncate(MAX_RECENT);
    rows
}

pub(super) fn row(recent: &Recent) -> Value {
    json!({"path":recent.path,"name":recent.path.file_name().map(|n| n.to_string_lossy().to_string()),"sources":recent.sources,"lastUsed":recent.last_used,"sites":recent.sites})
}

/// The native folder picker for this platform: program and arguments.
/// None where there is none (a Linux desktop without zenity or kdialog);
/// the panel then asks for a typed path.
pub(super) fn picker(
    prompt: &str,
    find: impl Fn(&str) -> Option<PathBuf>,
) -> Option<(PathBuf, Vec<String>)> {
    if cfg!(target_os = "macos") {
        // `activate` brings the dialog in front of Chrome.
        let script = format!(
            "activate\nPOSIX path of (choose folder with prompt \"{}\")",
            prompt.replace(['"', '\\'], "")
        );
        return Some((
            PathBuf::from("/usr/bin/osascript"),
            vec!["-e".into(), script],
        ));
    }
    if cfg!(windows) {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = '{}'; $d.ShowNewFolderButton = $false; if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.SelectedPath) }}",
            prompt.replace('\'', "''")
        );
        return Some((
            find("powershell.exe").unwrap_or_else(|| PathBuf::from("powershell.exe")),
            ["-NoProfile", "-NonInteractive", "-STA", "-Command", &script]
                .map(String::from)
                .to_vec(),
        ));
    }
    if let Some(zenity) = find("zenity") {
        return Some((
            zenity,
            vec![
                "--file-selection".into(),
                "--directory".into(),
                format!("--title={prompt}"),
            ],
        ));
    }
    find("kdialog").map(|kdialog| {
        (
            kdialog,
            vec![
                "--getexistingdirectory".into(),
                ".".into(),
                "--title".into(),
                prompt.into(),
            ],
        )
    })
}

/// What a picker printed: the chosen folder, or None when the user
/// cancelled (no output or a non-zero exit).
pub(super) fn picked(success: bool, stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let text = text.trim_end_matches(['\r', '\n']);
    let text = if text.len() > 1 {
        text.trim_end_matches(['/', '\\'])
    } else {
        text
    };
    (success && !text.is_empty()).then(|| text.to_string())
}

/// Open the folder picker and wait for the user. `{"path": …}` when a folder
/// was chosen (validated), `{"cancelled": true}`, or `{"unavailable": true}`
/// when this computer has no picker.
pub(super) async fn choose(prompt: &str, panel_root: &Path) -> Result<Value, String> {
    let Some((program, args)) = picker(prompt, |name| which(name)) else {
        return Ok(json!({"schemaVersion":1,"unavailable":true}));
    };
    let mut command = tokio::process::Command::new(&program);
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(PICK_TIMEOUT, command.output())
        .await
        .map_err(|_| "No folder was chosen in time")?
        .map_err(|_| "The folder picker could not be opened")?;
    match picked(output.status.success(), &output.stdout) {
        Some(path) => {
            let path = validate(&path, panel_root)?;
            Ok(json!({"schemaVersion":1,"path":path}))
        }
        None => Ok(json!({"schemaVersion":1,"cancelled":true})),
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = super::agent_search_path().or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_an_existing_folder_outside_system_and_panel_folders() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("app");
        std::fs::create_dir(&project).unwrap();
        let panel = dir.path().join("panel");
        std::fs::create_dir(&panel).unwrap();
        assert_eq!(
            validate(&project.to_string_lossy(), &panel).unwrap(),
            project.canonicalize().unwrap()
        );
        for bad in ["", "relative/app", "/no/such/folder/anywhere"] {
            assert!(validate(bad, &panel).is_err(), "{bad}");
        }
        let file = project.join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(validate(&file.to_string_lossy(), &panel).is_err());
        assert!(validate(&panel.join(".").to_string_lossy(), &panel).is_err());
        assert!(
            validate(&dir.path().to_string_lossy(), &panel).is_err(),
            "holds the panel data"
        );
        #[cfg(unix)]
        for system in ["/", "/usr", "/etc", "/tmp"] {
            assert!(validate(system, &panel).is_err(), "{system}");
        }
        if let Some(home) = dirs::home_dir() {
            assert!(
                validate(&home.to_string_lossy(), &panel).is_err(),
                "home itself"
            );
        }
    }

    #[test]
    fn recent_folders_merge_their_sources_and_drop_missing_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let rows = merge(
            [
                (a.clone(), "claude", 10, None),
                (b.clone(), "panel", 30, None),
                (
                    a.join("."),
                    "site",
                    20,
                    Some("http://localhost:3000".into()),
                ),
                (dir.path().join("gone"), "codex", 99, None),
                (PathBuf::from("/"), "codex", 98, None),
            ],
            &dir.path().join("panel"),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].path, b.canonicalize().unwrap());
        assert_eq!(rows[1].sources, ["claude", "site"]);
        assert_eq!(rows[1].last_used, 20);
        assert_eq!(rows[1].sites, ["http://localhost:3000"]);
        assert_eq!(row(&rows[1])["name"], "a");
    }

    #[test]
    fn pickers_and_their_answers() {
        let (program, args) = picker("Choose \"it\"", |_| None).unwrap_or_default();
        if cfg!(target_os = "macos") {
            assert_eq!(program, PathBuf::from("/usr/bin/osascript"));
            assert!(args[1].contains("choose folder with prompt \"Choose it\""));
        }
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert!(picker("x", |_| None).is_none(), "no picker: typed path");
            let (program, _) = picker("x", |n| {
                (n == "kdialog").then(|| PathBuf::from("/usr/bin/kdialog"))
            })
            .unwrap();
            assert_eq!(program, PathBuf::from("/usr/bin/kdialog"));
        }
        assert_eq!(
            picked(true, b"/Users/me/app/\n").as_deref(),
            Some("/Users/me/app")
        );
        assert_eq!(
            picked(true, b"C:\\work\\app").as_deref(),
            Some("C:\\work\\app")
        );
        assert_eq!(picked(true, b"/\n").as_deref(), Some("/"));
        assert_eq!(picked(false, b""), None, "cancelled");
        assert_eq!(picked(true, b"\n"), None);
    }
}
