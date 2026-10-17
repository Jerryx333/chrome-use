//! The macOS app bundle install.sh installs the binary as:
//! `<prefix>/share/chrome-use/ChromeUse.app`, with `<prefix>/bin/chrome-use`
//! a symlink to its `Contents/MacOS/chrome-use`.
//!
//! The bundle exists for macOS privacy settings: they key a bare executable
//! by its path and show it with no name or icon, while a process inside a
//! signed bundle is one "ChromeUse" entry, whatever path it runs from. Every
//! process started through the symlink already runs the bundle's image, so
//! only code that names the binary's location needs to know about it.

use std::path::{Path, PathBuf};

/// Whether `path` is the executable of an app bundle
/// (`<name>.app/Contents/MacOS/<file>`).
pub fn is_bundle_executable(path: &Path) -> bool {
    let Some(macos) = path.parent() else {
        return false;
    };
    let Some(contents) = macos.parent() else {
        return false;
    };
    macos.file_name().is_some_and(|n| n == "MacOS")
        && contents.file_name().is_some_and(|n| n == "Contents")
        && contents
            .parent()
            .and_then(Path::extension)
            .is_some_and(|e| e == "app")
}

/// The binary to name for a process that must run as the bundle: the
/// bundle's executable when `exe` resolves into one, `exe` itself otherwise.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn resolved(exe: &Path) -> PathBuf {
    match exe.canonicalize() {
        Ok(real) if is_bundle_executable(&real) => real,
        _ => exe.to_path_buf(),
    }
}

/// The directory the CLI is installed in, for an in-place upgrade. Called
/// through install.sh's symlink, that is the link's directory, not the
/// bundle's `Contents/MacOS` (install.sh would otherwise write over the
/// sealed executable). Run from the bundle directly (the panel service),
/// it is the `bin` next to the `share` folder that holds the bundle.
pub fn install_dir(exe: &Path) -> Option<PathBuf> {
    let real = exe.canonicalize().ok()?;
    if !is_bundle_executable(&real) {
        return real.parent().map(Path::to_path_buf);
    }
    if !is_bundle_executable(exe) {
        return exe.parent()?.canonicalize().ok();
    }
    let share = real.ancestors().nth(4)?;
    (share.file_name()? == "chrome-use" && share.parent()?.file_name()? == "share")
        .then(|| share.parent()?.parent().map(|prefix| prefix.join("bin")))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_bundle_executables_only() {
        assert!(is_bundle_executable(Path::new(
            "/x/ChromeUse.app/Contents/MacOS/chrome-use"
        )));
        assert!(!is_bundle_executable(Path::new("/x/bin/chrome-use")));
        assert!(!is_bundle_executable(Path::new(
            "/x/ChromeUse/Contents/MacOS/chrome-use"
        )));
        assert!(!is_bundle_executable(Path::new("chrome-use")));
    }

    #[cfg(unix)]
    #[test]
    fn install_dir_is_the_links_directory_or_the_prefix_bin() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let macos = root.join("share/chrome-use/ChromeUse.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("chrome-use");
        std::fs::write(&exe, b"").unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join("chrome-use");
        std::os::unix::fs::symlink(&exe, &link).unwrap();

        assert_eq!(install_dir(&link), Some(bin.clone()));
        assert_eq!(install_dir(&exe), Some(bin));
        assert_eq!(resolved(&link), exe);

        let bare = root.join("other/chrome-use");
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        std::fs::write(&bare, b"").unwrap();
        assert_eq!(install_dir(&bare), Some(root.join("other")));
        assert_eq!(resolved(&bare), bare);

        // A bundle somewhere else (the acceptance harness): no install dir.
        let loose = root.join("pa/ChromeUse.app/Contents/MacOS");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("chrome-use"), b"").unwrap();
        assert_eq!(install_dir(&loose.join("chrome-use")), None);
    }
}
