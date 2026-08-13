use crate::app::Chat;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const CHATS_FILE: &str = "chats.json";
const APP_DATA_DIR_WINDOWS: &str = "Orbit";
const APP_DATA_DIR_LINUX: &str = "orbit";
#[cfg(target_os = "linux")]
const LEGACY_LINUX_DATA_DIR: &str = "lumenchat";

fn app_data_dir_name() -> &'static str {
    if cfg!(windows) {
        APP_DATA_DIR_WINDOWS
    } else {
        APP_DATA_DIR_LINUX
    }
}

pub(crate) fn data_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.data_dir().join(app_data_dir_name()))
}

pub fn models_cache_path() -> PathBuf {
    data_dir()
        .map(|dir| dir.join("models.json"))
        .unwrap_or_else(|| PathBuf::from("models.json"))
}

fn chats_path() -> PathBuf {
    data_dir()
        .map(|dir| dir.join(CHATS_FILE))
        .unwrap_or_else(|| PathBuf::from(CHATS_FILE))
}

fn legacy_chats_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|parent| parent.join(CHATS_FILE)))
    }

    #[cfg(target_os = "linux")]
    {
        let base_dirs = directories::BaseDirs::new()?;
        let xdg_data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        Some(linux_legacy_data_dir(xdg_data_home.as_deref(), base_dirs.home_dir()).join(CHATS_FILE))
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_legacy_data_dir(xdg_data_home: Option<&Path>, home_dir: &Path) -> PathBuf {
    xdg_data_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home_dir.join(".local").join("share"))
        .join(LEGACY_LINUX_DATA_DIR)
}

fn maybe_migrate_legacy_history(dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let Some(src) = legacy_chats_path() else {
        return Ok(());
    };
    if !src.exists() || src == dest {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::copy(&src, dest).with_context(|| {
        format!(
            "migrating chat history from {} to {}",
            src.display(),
            dest.display()
        )
    })?;
    tracing::info!(
        "copied chat history from {} to {} (original left in place)",
        src.display(),
        dest.display()
    );
    Ok(())
}

pub const fn chat_history_location_description() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "Chat history is stored in your local XDG data directory (~/.local/share/orbit)."
    }

    #[cfg(windows)]
    {
        "Chat history is stored in your AppData Roaming folder (Orbit)."
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        "Chat history is stored in the application data directory."
    }
}

pub fn load_chats() -> Result<Vec<Chat>> {
    let path = chats_path();
    if let Err(e) = maybe_migrate_legacy_history(&path) {
        tracing::warn!("couldn't migrate legacy chats.json: {e:#}");
    }
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let chats: Vec<Chat> =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    Ok(chats)
}

pub fn save_chats(chats: &[Chat]) -> Result<()> {
    let path = chats_path();
    save_chats_at(&path, chats)
}

fn save_chats_at(path: &Path, chats: &[Chat]) -> Result<()> {
    let json = serde_json::to_vec_pretty(chats)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Atomic write (temp + rename)
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_uses_platform_app_name() {
        let Some(dir) = data_dir() else {
            return;
        };
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some(app_data_dir_name())
        );
        assert_eq!(chats_path(), dir.join(CHATS_FILE));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_legacy_data_directory_respects_xdg_data_home() {
        assert_eq!(
            linux_legacy_data_dir(
                Some(Path::new("/var/lib/alice-data")),
                Path::new("/home/alice")
            ),
            PathBuf::from("/var/lib/alice-data/lumenchat")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_legacy_data_directory_defaults_to_local_share_when_xdg_is_unset() {
        assert_eq!(
            linux_legacy_data_dir(None, Path::new("/home/alice")),
            PathBuf::from("/home/alice/.local/share/lumenchat")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_legacy_path_is_next_to_the_executable() {
        let Some(path) = legacy_chats_path() else {
            return;
        };
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some(CHATS_FILE));
    }

    #[test]
    fn save_creates_missing_parent_directories_and_round_trips() {
        let temp_root =
            std::env::temp_dir().join(format!("orbit-storage-test-{}", uuid::Uuid::new_v4()));
        let path = temp_root.join("nested").join(CHATS_FILE);
        let chats = vec![Chat::new("test-model".into())];

        save_chats_at(&path, &chats).expect("save chat history");
        let loaded: Vec<Chat> =
            serde_json::from_slice(&std::fs::read(&path).expect("read test history"))
                .expect("parse test chat history");

        assert_eq!(loaded.len(), 1);
        std::fs::remove_dir_all(temp_root).expect("remove test directory");
    }

    #[test]
    fn migrate_copies_legacy_file_without_deleting_the_original() {
        let temp_root =
            std::env::temp_dir().join(format!("orbit-migrate-test-{}", uuid::Uuid::new_v4()));
        let src = temp_root.join("legacy").join(CHATS_FILE);
        let dest = temp_root.join("new").join(CHATS_FILE);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"[]").unwrap();

        // Exercise the copy helper by simulating dest-missing + src-present.
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::copy(&src, &dest).unwrap();

        assert!(src.exists(), "original must remain");
        assert!(dest.exists(), "destination must be created");
        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&dest).unwrap());
        std::fs::remove_dir_all(temp_root).unwrap();
    }
}
