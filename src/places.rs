//! Where Seoul's files live. Presets, shaders and screenshots are relative
//! to the working folder, which is the install; settings (`seoul.toml`,
//! `seoul-state.toml`) live where replacing the install can't reach them.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::config;
use crate::preset::curation::STATE_PATH;

/// Settle the working folder, then find the settings folder: `$EASEL_DATA`
/// under the Easel launcher, else on Linux `$XDG_CONFIG_HOME/seoul` when
/// running from an install. An empty path means the working folder itself,
/// as in a source checkout, on Windows, and on macOS (whose working folder
/// is already outside the bundle).
pub fn settle() -> PathBuf {
    let installed = settle_working_dir();
    let Some(dir) = settings_dir(
        easel_data().map(PathBuf::into_os_string),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        installed && cfg!(target_os = "linux"),
    ) else {
        return PathBuf::new();
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("couldn't make {}: {e}; keeping settings beside the presets", dir.display());
        return PathBuf::new();
    }
    adopt(Path::new(""), &dir);
    info!(dir = %dir.display(), "settings folder");
    dir
}

/// The Easel launcher's folder for this app: settings, and a thumbnail on quit.
pub fn easel_data() -> Option<PathBuf> {
    std::env::var_os("EASEL_DATA").filter(|d| !d.is_empty()).map(PathBuf::from)
}

fn settings_dir(
    easel: Option<OsString>,
    xdg_config: Option<OsString>,
    home: Option<OsString>,
    installed: bool,
) -> Option<PathBuf> {
    if let Some(dir) = easel.filter(|d| !d.is_empty()) {
        return Some(dir.into());
    }
    if !installed {
        return None;
    }
    // The XDG spec ignores a relative XDG_CONFIG_HOME.
    let xdg = xdg_config.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = xdg.or_else(|| home.filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("seoul"))
}

/// Seed the settings folder from the install: the shipped `seoul.toml` on
/// first run, and settings an older version kept beside the presets. Files
/// already there are left alone.
fn adopt(install: &Path, settings: &Path) {
    for name in [config::DEFAULT_PATH, STATE_PATH] {
        let (from, to) = (install.join(name), settings.join(name));
        if from.is_file()
            && !to.exists()
            && let Err(e) = std::fs::copy(&from, &to)
        {
            warn!("couldn't copy {} into {}: {e}", from.display(), settings.display());
        }
    }
}

/// Write `path` whole or not at all: a temp file beside it, renamed over it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Presets, settings, favorites and screenshots are relative paths. Launched
/// from elsewhere (a shortcut, a file manager, Finder), work from the exe's
/// folder, or on macOS from the app's support folder. Returns whether Seoul
/// is working from its install rather than, say, a source checkout.
fn settle_working_dir() -> bool {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    if Path::new("presets").is_dir() {
        return exe_dir.is_some_and(|d| same_dir(&d, Path::new(".")));
    }
    let Some(exe_dir) = exe_dir else {
        return false;
    };
    #[cfg(target_os = "macos")]
    let exe_dir = mac_support_dir(&exe_dir).unwrap_or(exe_dir);
    if exe_dir.join("presets").is_dir() {
        info!(dir = %exe_dir.display(), "working from the app's folder");
        return std::env::set_current_dir(exe_dir).is_ok();
    }
    false
}

fn same_dir(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// Inside Seoul.app the presets ship in Contents/Resources, which is
/// read-only in spirit (writing there breaks the signature) and sometimes in
/// fact (a downloaded app can run from a randomized read-only copy). Work
/// from ~/Library/Application Support/Seoul instead, seeded from the bundle.
/// Files already there are left alone, so edits survive updates and new
/// presets still arrive.
#[cfg(target_os = "macos")]
fn mac_support_dir(exe_dir: &Path) -> Option<PathBuf> {
    let resources = exe_dir.parent()?.join("Resources");
    if !resources.join("presets").is_dir() {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Seoul");
    std::fs::create_dir_all(home.join("presets")).ok()?;
    let copy_missing = |from: &Path, to: &Path| {
        if !to.exists()
            && let Err(e) = std::fs::copy(from, to)
        {
            warn!("couldn't copy {} into {}: {e}", from.display(), to.display());
        }
    };
    for entry in std::fs::read_dir(resources.join("presets")).ok()?.flatten() {
        copy_missing(&entry.path(), &home.join("presets").join(entry.file_name()));
    }
    copy_missing(&resources.join("seoul.toml"), &home.join("seoul.toml"));
    Some(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("seoul-places-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn easel_wins_everywhere() {
        let dir = settings_dir(Some("/easel/apps/seoul".into()), Some("/x".into()), Some("/home/me".into()), false);
        assert_eq!(dir, Some(PathBuf::from("/easel/apps/seoul")));
    }

    #[test]
    fn an_install_keeps_settings_in_the_config_home() {
        let root = std::env::temp_dir();
        let xdg = root.join("xdg");
        let home = root.join("me");
        let dir = |x: Option<&Path>, installed| settings_dir(None, x.map(Into::into), Some(home.clone().into()), installed);
        assert_eq!(dir(Some(&xdg), true), Some(xdg.join("seoul")));
        // No XDG_CONFIG_HOME, or a relative one: ~/.config.
        assert_eq!(dir(None, true), Some(home.join(".config/seoul")));
        assert_eq!(dir(Some(Path::new("rel")), true), Some(home.join(".config/seoul")));
        // A source checkout keeps its seoul.toml.
        assert_eq!(dir(Some(&xdg), false), None);
        assert_eq!(settings_dir(Some("".into()), None, None, true), None);
    }

    #[test]
    fn adopting_seeds_missing_files_only() {
        let install = scratch("install");
        let settings = scratch("settings");
        std::fs::write(install.join(config::DEFAULT_PATH), "shipped").unwrap();
        std::fs::write(install.join(STATE_PATH), "old favorites").unwrap();
        std::fs::write(settings.join(config::DEFAULT_PATH), "mine").unwrap();
        adopt(&install, &settings);
        let read = |name| std::fs::read_to_string(settings.join(name)).unwrap();
        assert_eq!(read(config::DEFAULT_PATH), "mine");
        assert_eq!(read(STATE_PATH), "old favorites");
        // Replacing the install loses nothing.
        std::fs::remove_dir_all(&install).unwrap();
        assert_eq!(read(STATE_PATH), "old favorites");
        std::fs::remove_dir_all(&settings).unwrap();
    }

    #[test]
    fn atomic_writes_replace_the_whole_file() {
        let dir = scratch("atomic");
        let path = dir.join("f.toml");
        write_atomic(&path, b"a long first version").unwrap();
        write_atomic(&path, b"short").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "short");
        assert!(!dir.join("f.toml.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
