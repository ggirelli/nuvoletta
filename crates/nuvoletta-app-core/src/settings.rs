//! Non-secret application settings, stored as a small JSON file outside the vault.

use crate::error::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Default inactivity period before the vault locks itself.
pub const DEFAULT_AUTO_LOCK_MINUTES: u32 = 5;
/// Largest accepted auto-lock period (24 hours). There is deliberately no "never".
pub const MAX_AUTO_LOCK_MINUTES: u32 = 24 * 60;

/// Application settings. Contains no secrets: only the auto-lock period and the last vault
/// directory (a path, so the picker can offer it again).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Minutes of inactivity before auto-lock, in `1..=MAX_AUTO_LOCK_MINUTES`.
    pub auto_lock_minutes: u32,
    /// Directory of the vault used last.
    pub last_vault: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
            last_vault: None,
        }
    }
}

impl Settings {
    /// Clamp the auto-lock period into its valid range.
    #[must_use]
    pub fn clamp_minutes(minutes: u32) -> u32 {
        minutes.clamp(1, MAX_AUTO_LOCK_MINUTES)
    }

    /// Load settings from `path`. A missing, unreadable or malformed file yields the defaults;
    /// out-of-range values are clamped. Never fails: settings must not block unlocking.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let mut s = Self::default();
        let Ok(text) = fs::read_to_string(path) else {
            return s;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return s;
        };
        if let Some(m) = v
            .get("auto_lock_minutes")
            .and_then(serde_json::Value::as_u64)
        {
            s.auto_lock_minutes = Self::clamp_minutes(u32::try_from(m).unwrap_or(u32::MAX));
        }
        if let Some(p) = v.get("last_vault").and_then(serde_json::Value::as_str) {
            s.last_vault = Some(PathBuf::from(p));
        }
        s
    }

    /// Write the settings to `path` (temporary file, then rename).
    ///
    /// # Errors
    /// [`Error::Settings`] if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        let v = serde_json::json!({
            "auto_lock_minutes": self.auto_lock_minutes,
            "last_vault": self.last_vault.as_ref().map(|p| p.to_string_lossy().into_owned()),
        });
        let err = |e: std::io::Error| Error::Settings(e.to_string());
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(err)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, v.to_string()).map_err(err)?;
        fs::rename(&tmp, path).map_err(err)
    }
}

/// Split user input such as `#work, ideas  todo` into clean tags: separators are commas and
/// whitespace, a leading `#` is dropped, empty and duplicate (case-insensitive) tags are removed.
#[must_use]
pub fn parse_tags(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in input.split(|c: char| c == ',' || c.is_whitespace()) {
        let t = raw.trim().trim_start_matches('#');
        if !t.is_empty() && !out.iter().any(|o| o.to_lowercase() == t.to_lowercase()) {
            out.push(t.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_malformed_file_gives_defaults() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load(&d.path().join("none.json")),
            Settings::default()
        );
        let p = d.path().join("bad.json");
        fs::write(&p, "{not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn settings_round_trip_and_are_clamped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/settings.json");
        let s = Settings {
            auto_lock_minutes: 12,
            last_vault: Some("/some/vault".into()),
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        fs::write(&p, r#"{"auto_lock_minutes": 0}"#).unwrap();
        assert_eq!(Settings::load(&p).auto_lock_minutes, 1);
        fs::write(&p, r#"{"auto_lock_minutes": 99999999}"#).unwrap();
        assert_eq!(Settings::load(&p).auto_lock_minutes, MAX_AUTO_LOCK_MINUTES);
    }

    #[test]
    fn tags_are_split_cleaned_and_deduplicated() {
        assert_eq!(
            parse_tags("#work, ideas  Work todo,"),
            ["work", "ideas", "todo"]
        );
        assert!(parse_tags(" , # ").is_empty());
    }
}
