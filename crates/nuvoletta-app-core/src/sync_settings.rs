//! Per-vault, per-device sync settings: whether sync is on and which local folder is the
//! externally synced one. Stored in `<vault>/.nuvoletta/device-state/sync.json`, never inside
//! synced data and never uploaded (`docs/STORAGE_FORMAT.md`). No secrets.

use crate::error::{Error, Result};
use nuvoletta_storage::fsutil::write_atomic;
use nuvoletta_storage::layout::META_DIR;
use std::fs;
use std::path::{Path, PathBuf};

/// Directory of device-local state inside `.nuvoletta/`.
pub(crate) const DEVICE_STATE_DIR: &str = "device-state";
const FILE: &str = "sync.json";

/// Sync settings of one vault on this device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncSettings {
    /// Sync runs only while this is set (and a folder is chosen).
    pub enabled: bool,
    /// The externally synced folder.
    pub folder: Option<PathBuf>,
}

pub(crate) fn device_state_dir(vault_root: &Path) -> PathBuf {
    vault_root.join(META_DIR).join(DEVICE_STATE_DIR)
}

/// Write a small file into `device-state/` atomically.
pub(crate) fn write_device_file(vault_root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let dir = device_state_dir(vault_root);
    fs::create_dir_all(&dir).map_err(|e| Error::Settings(e.to_string()))?;
    write_atomic(&dir, name, bytes).map_err(|e| Error::Settings(e.to_string()))
}

impl SyncSettings {
    /// Load the settings of the vault at `vault_root`. A missing or malformed file means "sync
    /// off": settings must never block opening a vault.
    #[must_use]
    pub fn load(vault_root: &Path) -> Self {
        let Ok(text) = fs::read_to_string(device_state_dir(vault_root).join(FILE)) else {
            return Self::default();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self::default();
        };
        Self {
            enabled: v
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            folder: v
                .get("folder")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from),
        }
    }

    /// Write the settings.
    ///
    /// # Errors
    /// [`Error::Settings`] if the file cannot be written.
    pub fn save(&self, vault_root: &Path) -> Result<()> {
        let v = serde_json::json!({
            "enabled": self.enabled,
            "folder": self.folder.as_ref().map(|p| p.to_string_lossy().into_owned()),
        });
        write_device_file(vault_root, FILE, v.to_string().as_bytes())
    }

    /// Whether sync should run.
    #[must_use]
    pub fn active(&self) -> bool {
        self.enabled && self.folder.is_some()
    }
}

/// Check that `folder` can be the sync folder of the vault at `vault_root`: an existing
/// directory that neither contains nor lies inside the vault (so vault files can never be mixed
/// with protocol files).
///
/// # Errors
/// [`Error::Invalid`] with an explanation.
pub fn check_sync_folder(vault_root: &Path, folder: &Path) -> Result<PathBuf> {
    let folder = fs::canonicalize(folder)
        .map_err(|_| Error::Invalid("That folder does not exist or cannot be read."))?;
    if !folder.is_dir() {
        return Err(Error::Invalid("The sync folder must be a folder."));
    }
    let vault = fs::canonicalize(vault_root)
        .map_err(|_| Error::Invalid("The vault folder cannot be read."))?;
    if folder.starts_with(&vault) || vault.starts_with(&folder) {
        return Err(Error::Invalid(
            "The sync folder must be separate from the vault folder: choose a different folder.",
        ));
    }
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_default_to_off() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(SyncSettings::load(d.path()), SyncSettings::default());
        let s = SyncSettings {
            enabled: true,
            folder: Some("/some/folder".into()),
        };
        s.save(d.path()).unwrap();
        assert_eq!(SyncSettings::load(d.path()), s);
        assert!(s.active());
        fs::write(device_state_dir(d.path()).join(FILE), "{broken").unwrap();
        assert_eq!(SyncSettings::load(d.path()), SyncSettings::default());
    }

    #[test]
    fn folder_must_exist_and_be_separate_from_the_vault() {
        let d = tempfile::tempdir().unwrap();
        let vault = d.path().join("vault");
        let other = d.path().join("shared");
        fs::create_dir_all(vault.join("inner")).unwrap();
        fs::create_dir_all(&other).unwrap();
        assert!(check_sync_folder(&vault, &other).is_ok());
        assert!(check_sync_folder(&vault, &d.path().join("missing")).is_err());
        assert!(check_sync_folder(&vault, &vault).is_err());
        assert!(check_sync_folder(&vault, &vault.join("inner")).is_err());
        assert!(check_sync_folder(&vault.join("inner"), d.path()).is_err());
    }
}
