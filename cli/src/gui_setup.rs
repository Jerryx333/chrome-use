//! Finder-launch setup: ChromeUse.app opened by double-click installs itself.
//!
//! A user who added the extension from the Chrome Web Store has no CLI yet,
//! so the panel cannot connect. The extension's setup guide downloads
//! `chrome-use-darwin-<arch>.dmg` (signed, notarized, stapled; built by
//! release-binaries.yml with scripts/macos-dmg.sh). Opening the ChromeUse
//! app inside it from Finder lands here instead of printing help to nowhere:
//!
//! 1. copy the bundle to `<prefix>/share/chrome-use/ChromeUse.app` and link
//!    `<prefix>/bin/chrome-use` to it, exactly as install.sh lays it out, so
//!    `upgrade`, the auto-upgrade and `app_bundle::install_dir` work the same;
//!    `<prefix>` is `/usr/local` when its `bin` is writable, else `~/.local`;
//! 2. run the installed copy's `extension install --json --no-profile
//!    --no-open`, which writes the native-messaging host manifests (never the
//!    force-install policy profile, which would put Chrome in managed mode);
//! 3. run its `skill install` so the user's agents learn chrome-use;
//! 4. say what happened in a dialog.
//!
//! The installed copy, not the running one, registers the host: opened from
//! a quarantined disk image, this process runs from an App Translocation
//! path that disappears when the image is ejected.
//!
//! No admin password, no terminal. The extension polls the host while its
//! setup guide is showing and continues on its own once it answers.
//!
//! Detection is narrow on purpose (see [`is_finder_launch`]): no arguments,
//! running as a bundle executable, LaunchServices' `__CFBundleIdentifier`
//! naming this bundle, and no terminal on stdin. A script running
//! `chrome-use` with no arguments still gets the help text.
//!
//! `chrome-use __gui-setup [--no-dialog] [extension install args…]` runs the
//! same steps by hand (the acceptance rig times it with a private HOME).

use std::path::{Path, PathBuf};

/// `CFBundleIdentifier` in scripts/macos-app-bundle.sh.
pub const BUNDLE_ID: &str = "com.leeguoo.chrome-use";

const LOG_NAME: &str = "gui-setup.log";

/// Whether this process is ChromeUse.app opened from Finder (or `open`).
/// `args` excludes argv\[0\]; older macOS passes a `-psn_…` argument.
pub fn is_finder_launch(
    args: &[String],
    exe: &Path,
    bundle_env: Option<&str>,
    stdin_tty: bool,
) -> bool {
    let no_args = match args {
        [] => true,
        [only] => only.starts_with("-psn_"),
        _ => false,
    };
    no_args
        && !stdin_tty
        && bundle_env == Some(BUNDLE_ID)
        && crate::app_bundle::is_bundle_executable(exe)
}

/// The same check for the running process.
pub fn finder_launch_now() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    use std::io::IsTerminal;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    is_finder_launch(
        &args,
        &exe,
        std::env::var("__CFBundleIdentifier").ok().as_deref(),
        std::io::stdin().is_terminal(),
    )
}

/// install.sh's choice: `AGENT_BROWSER_BIN_DIR`, else `/usr/local/bin` when
/// it and `/usr/local/share/chrome-use` are writable (`usr_local_writable`),
/// else `~/.local/bin`. Returns `(bin dir, bundle path)`.
pub fn install_layout(
    bin_dir_env: Option<&Path>,
    usr_local_bin_writable: bool,
    home: &Path,
) -> (PathBuf, PathBuf) {
    let bin = match bin_dir_env {
        Some(dir) => dir.to_path_buf(),
        None if usr_local_bin_writable => PathBuf::from("/usr/local/bin"),
        None => home.join(".local/bin"),
    };
    let prefix = bin
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| bin.clone());
    let app = prefix.join("share/chrome-use/ChromeUse.app");
    (bin, app)
}

#[derive(Debug, PartialEq, Eq)]
pub enum BundleAction {
    /// Copy the running bundle into place (new install, or ours is newer or equal).
    Copy,
    /// Keep what is installed: it is the running bundle, or newer.
    Keep,
}

/// What to do with the bundle already at the install path, if any. Opening
/// an old disk image must not downgrade a CLI that upgraded itself since.
pub fn bundle_action(same_bundle: bool, ours: &str, installed: Option<&str>) -> BundleAction {
    if same_bundle {
        return BundleAction::Keep;
    }
    match installed {
        Some(theirs) if crate::upgrade::version_is_newer(theirs, ours) => BundleAction::Keep,
        _ => BundleAction::Copy,
    }
}

/// `chrome-use __gui-setup [--no-dialog] [extension install args…]`.
pub fn run_cli(args: &[String]) -> i32 {
    let dialog = !args.iter().any(|a| a == "--no-dialog");
    let forwarded: Vec<String> = args
        .iter()
        .filter(|a| *a != "--no-dialog")
        .cloned()
        .collect();
    run(&forwarded, dialog)
}

/// Run the setup; returns the exit code.
pub fn run(extra_install_args: &[String], dialog: bool) -> i32 {
    let mut log = Log::open();
    let result = setup(extra_install_args, &mut log);
    match &result {
        Ok(done) => {
            log.line(&format!("ok: {}", done.summary()));
            if dialog {
                show_dialog(&done.message(), false);
            } else {
                println!("{}", done.summary());
            }
            0
        }
        Err(error) => {
            log.line(&format!("failed: {error}"));
            if dialog {
                show_dialog(&failure_message(error), true);
            } else {
                eprintln!("chrome-use setup failed: {error}");
            }
            1
        }
    }
}

struct Done {
    bin: PathBuf,
    app: PathBuf,
    kept_newer: Option<String>,
    skill_warning: Option<String>,
}

impl Done {
    fn summary(&self) -> String {
        let mut s = format!(
            "installed {} -> {}; native host registered",
            self.bin.display(),
            self.app.display()
        );
        if let Some(v) = &self.kept_newer {
            s.push_str(&format!(" (kept the newer {v} already installed)"));
        }
        if let Some(w) = &self.skill_warning {
            s.push_str(&format!("; skill install: {w}"));
        }
        s
    }

    fn message(&self) -> String {
        let zh = crate::connect::ui_zh();
        if zh {
            "chrome-use 已装好。\n\n回到 Chrome，打开 chrome-use 侧边栏，它会自己连上。这个安装窗口和磁盘映像可以关掉了。".to_string()
        } else {
            "chrome-use is installed.\n\nGo back to Chrome and open the chrome-use side panel; it connects on its own. You can close this window and eject the disk image.".to_string()
        }
    }
}

fn failure_message(error: &str) -> String {
    let log = log_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if crate::connect::ui_zh() {
        format!(
            "chrome-use 没装好：{error}\n\n可以改用终端安装：\ncurl -fsSL https://raw.githubusercontent.com/leeguooooo/chrome-use/main/install.sh | sh\n\n日志：{log}"
        )
    } else {
        format!(
            "chrome-use could not finish setting up: {error}\n\nInstall from a terminal instead:\ncurl -fsSL https://raw.githubusercontent.com/leeguooooo/chrome-use/main/install.sh | sh\n\nLog: {log}"
        )
    }
}

fn setup(extra_install_args: &[String], log: &mut Log) -> Result<Done, String> {
    if !cfg!(target_os = "macos") {
        return Err("the double-click setup is for macOS; use install.sh or install.ps1".into());
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if !crate::app_bundle::is_bundle_executable(&exe) {
        return Err(format!("{} is not inside ChromeUse.app", exe.display()));
    }
    let src_app = exe
        .ancestors()
        .nth(3)
        .ok_or("no bundle around this program")?
        .to_path_buf();
    let home = dirs::home_dir().ok_or("no home directory")?;
    let bin_env = std::env::var_os("AGENT_BROWSER_BIN_DIR").map(PathBuf::from);
    // install.sh uses /usr/local only when its bin is writable AND the
    // bundle's share directory can be made and written (Homebrew owns
    // /usr/local/bin on Intel Macs, not always /usr/local/share).
    let usr_local = bin_env.is_none() && dir_writable(Path::new("/usr/local/bin")) && {
        let share = Path::new("/usr/local/share/chrome-use");
        std::fs::create_dir_all(share).is_ok() && dir_writable(share)
    };
    let (bin_dir, app) = install_layout(bin_env.as_deref(), usr_local, &home);
    log.line(&format!("from {} to {}", src_app.display(), app.display()));

    let same = app.canonicalize().ok().as_deref() == Some(src_app.as_path());
    let ours = env!("CARGO_PKG_VERSION");
    let installed = bundle_version(&app);
    let mut kept_newer = None;
    match bundle_action(same, ours, installed.as_deref()) {
        BundleAction::Keep => {
            if !same {
                kept_newer = installed.clone();
            }
            log.line(&format!("keeping installed bundle {installed:?}"));
        }
        BundleAction::Copy => copy_bundle(&src_app, &app, log)?,
    }

    let target = app.join("Contents/MacOS/chrome-use");
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("cannot create {}: {e}", bin_dir.display()))?;
    link_into(&bin_dir.join("chrome-use"), &target)?;
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    let _ = std::process::Command::new(lsregister)
        .arg("-f")
        .arg(&app)
        .output();

    // The installed copy registers the host, so the manifests name a path that
    // outlives this (possibly translocated) process.
    let mut install = vec![
        "extension",
        "install",
        "--json",
        "--no-profile",
        "--no-open",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    install.extend(extra_install_args.iter().cloned());
    let out = std::process::Command::new(&target)
        .args(&install)
        .env("CHROME_USE_NO_UPDATE_CHECK", "1")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", target.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    log.line(&format!("extension install: {}", stdout.trim()));
    let reply: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_default();
    if !out.status.success() || reply["success"] != serde_json::Value::Bool(true) {
        let reason = reply["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_string());
        return Err(format!("registering the native host failed: {reason}"));
    }

    let skill = std::process::Command::new(&target)
        .args(["skill", "install"])
        .env("CHROME_USE_NO_UPDATE_CHECK", "1")
        .output();
    let skill_warning = match skill {
        Ok(o) if o.status.success() => None,
        Ok(o) => Some(
            String::from_utf8_lossy(&o.stderr)
                .trim()
                .chars()
                .take(300)
                .collect(),
        ),
        Err(e) => Some(e.to_string()),
    };
    if let Some(w) = &skill_warning {
        log.line(&format!("skill install warning: {w}"));
    }
    Ok(Done {
        bin: bin_dir.join("chrome-use"),
        app,
        kept_newer,
        skill_warning,
    })
}

fn dir_writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
    }
}

/// `CFBundleShortVersionString` of an installed bundle.
fn bundle_version(app: &Path) -> Option<String> {
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
    let after = plist
        .split("<key>CFBundleShortVersionString</key>")
        .nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")?;
    Some(after[start..start + end].trim().to_string())
}

/// Replace `dest` by a copy of `src`, by renames, like install.sh: a running
/// chrome-use keeps executing the old image.
fn copy_bundle(src: &Path, dest: &Path, log: &mut Log) -> Result<(), String> {
    let parent = dest.parent().ok_or("bad install path")?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let staged = parent.join("ChromeUse.app.new");
    let _ = std::fs::remove_dir_all(&staged);
    // ditto keeps the code signature and the stapled ticket.
    let status = std::process::Command::new("ditto")
        .arg(src)
        .arg(&staged)
        .status()
        .map_err(|e| format!("cannot run ditto: {e}"))?;
    if !status.success() {
        return Err(format!("copying the app to {} failed", parent.display()));
    }
    // The copy inherits the download's quarantine flag. Gatekeeper already
    // assessed this notarized bundle when the user opened it; install.sh's
    // curl download never carries the flag either.
    let _ = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&staged)
        .status();
    let old = parent.join(format!("ChromeUse.app.old.{}", std::process::id()));
    if dest.exists() {
        std::fs::rename(dest, &old).map_err(|e| format!("cannot move the old app aside: {e}"))?;
    }
    std::fs::rename(&staged, dest).map_err(|e| format!("cannot put the app in place: {e}"))?;
    let _ = std::fs::remove_dir_all(&old);
    log.line(&format!("copied bundle to {}", dest.display()));
    Ok(())
}

/// Point `link` at `target` atomically (a temporary link, then a rename).
fn link_into(link: &Path, target: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let tmp = link.with_file_name(format!(".chrome-use.link.{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(target, &tmp)
            .map_err(|e| format!("cannot link {}: {e}", link.display()))?;
        std::fs::rename(&tmp, link).map_err(|e| format!("cannot link {}: {e}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (link, target);
        Err("not supported on this platform".into())
    }
}

fn show_dialog(text: &str, failed: bool) {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let icon = if failed { "caution" } else { "note" };
    let script = format!(
        "display dialog \"{escaped}\" with title \"chrome-use\" buttons {{\"OK\"}} default button \"OK\" with icon {icon} giving up after 600"
    );
    let _ = std::process::Command::new("osascript")
        .args(["-e", &script])
        .status();
}

fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".chrome-use").join(LOG_NAME))
}

struct Log(Option<std::fs::File>);

impl Log {
    fn open() -> Self {
        let file = log_path().and_then(|p| {
            std::fs::create_dir_all(p.parent()?).ok()?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        Log(file)
    }

    fn line(&mut self, text: &str) {
        use std::io::Write;
        if let Some(f) = self.0.as_mut() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "{now} {text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str =
        "/private/var/folders/x/AppTranslocation/1/d/ChromeUse.app/Contents/MacOS/chrome-use";

    #[test]
    fn finder_launch_needs_every_signal() {
        let exe = Path::new(EXE);
        assert!(is_finder_launch(&[], exe, Some(BUNDLE_ID), false));
        assert!(is_finder_launch(
            &["-psn_0_12345".into()],
            exe,
            Some(BUNDLE_ID),
            false
        ));
        // A terminal, a script with arguments, another app's environment
        // (Terminal sets com.apple.Terminal), or a bare binary: no setup.
        assert!(!is_finder_launch(&[], exe, Some(BUNDLE_ID), true));
        assert!(!is_finder_launch(
            &["open".into()],
            exe,
            Some(BUNDLE_ID),
            false
        ));
        assert!(!is_finder_launch(
            &[],
            exe,
            Some("com.apple.Terminal"),
            false
        ));
        assert!(!is_finder_launch(&[], exe, None, false));
        assert!(!is_finder_launch(
            &[],
            Path::new("/usr/local/bin/chrome-use"),
            Some(BUNDLE_ID),
            false
        ));
    }

    #[test]
    fn layout_matches_install_sh() {
        let home = Path::new("/Users/u");
        assert_eq!(
            install_layout(None, false, home),
            (
                PathBuf::from("/Users/u/.local/bin"),
                PathBuf::from("/Users/u/.local/share/chrome-use/ChromeUse.app")
            )
        );
        assert_eq!(
            install_layout(None, true, home),
            (
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/local/share/chrome-use/ChromeUse.app")
            )
        );
        assert_eq!(
            install_layout(Some(Path::new("/opt/x/bin")), true, home),
            (
                PathBuf::from("/opt/x/bin"),
                PathBuf::from("/opt/x/share/chrome-use/ChromeUse.app")
            )
        );
    }

    #[test]
    fn never_downgrades_an_installed_bundle() {
        assert_eq!(bundle_action(false, "1.5.272", None), BundleAction::Copy);
        assert_eq!(
            bundle_action(false, "1.5.272", Some("1.5.200")),
            BundleAction::Copy
        );
        assert_eq!(
            bundle_action(false, "1.5.272", Some("1.5.272")),
            BundleAction::Copy
        );
        assert_eq!(
            bundle_action(false, "1.5.272", Some("1.5.280")),
            BundleAction::Keep
        );
        assert_eq!(
            bundle_action(true, "1.5.272", Some("1.5.272")),
            BundleAction::Keep
        );
    }

    #[test]
    fn reads_the_bundle_version() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("ChromeUse.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            "<dict>\n\t<key>CFBundleShortVersionString</key><string>1.5.271</string>\n</dict>",
        )
        .unwrap();
        assert_eq!(bundle_version(&app).as_deref(), Some("1.5.271"));
        assert_eq!(bundle_version(&dir.path().join("none.app")), None);
    }
}
