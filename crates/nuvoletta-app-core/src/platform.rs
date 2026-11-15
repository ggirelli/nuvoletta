//! Platform facts that do not need a webview: where the Android app keeps its private files,
//! and what the Android build cannot do yet. Pure functions so they can be tested anywhere.

use std::path::PathBuf;

/// Android application id (also the app's private directory name). Must match
/// `apps/nuvoletta-app/Dioxus.toml`.
pub const ANDROID_APP_ID: &str = "dev.nuvoletta.app";

/// Shown in place of the sync controls on Android (`docs/ADR/0006-android-app.md`).
pub const ANDROID_SYNC_UNAVAILABLE: &str = "Sync is not available on Android yet. The synced-folder backend needs a folder an \
     external sync app can reach, which Android only offers through the Storage Access \
     Framework; that is not implemented. Your notes stay encrypted in the app's private \
     storage on this device.";

/// Android multi-user: user `n` owns uids `n * 100_000 ..`.
const ANDROID_UID_PER_USER: u32 = 100_000;

/// Extract the application id from `/proc/self/cmdline` (the zygote names the process after the
/// package, optionally followed by `:service` for secondary processes). Returns `None` if the
/// text is not a plausible package name, so a hostile or odd value never builds a path.
#[must_use]
pub fn package_from_cmdline(cmdline: &[u8]) -> Option<String> {
    let first = cmdline.split(|b| *b == 0).next()?;
    let name = std::str::from_utf8(first).ok()?;
    let name = name.split(':').next()?;
    let ok = !name.is_empty()
        && name.len() <= 255
        && name.contains('.')
        && name
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    ok.then(|| name.to_owned())
}

/// The app-private `files` directory (`Context.getFilesDir()`) derived from the process uid and
/// package name: `/data/user/<user>/<package>/files`. Done without JNI so no `unsafe` is needed.
#[must_use]
pub fn android_files_dir(package: &str, uid: u32) -> PathBuf {
    PathBuf::from("/data/user")
        .join((uid / ANDROID_UID_PER_USER).to_string())
        .join(package)
        .join("files")
}

/// Where the Android build keeps settings and the vault, below the private files directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidPaths {
    /// `settings.json` (auto-lock period, last vault; no secrets).
    pub settings: PathBuf,
    /// The vault directory. Fixed: the vault never leaves app-private storage on Android.
    pub vault: PathBuf,
}

impl AndroidPaths {
    /// Layout below a private files directory.
    #[must_use]
    pub fn in_files_dir(files: &std::path::Path) -> Self {
        Self {
            settings: files.join("settings.json"),
            vault: files.join("vault"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_name_is_taken_from_the_process_name() {
        assert_eq!(
            package_from_cmdline(b"dev.nuvoletta.app\0\0\0").as_deref(),
            Some("dev.nuvoletta.app")
        );
        assert_eq!(
            package_from_cmdline(b"dev.nuvoletta.app:svc\0").as_deref(),
            Some("dev.nuvoletta.app")
        );
    }

    #[test]
    fn implausible_process_names_are_refused() {
        for bad in [
            &b""[..],
            b"\0",
            b"nodots",
            b"../../etc\0",
            b"a..b\0",
            b"a.b/c\0",
            b"a.b c\0",
            b"\xff\xfe.x\0",
        ] {
            assert_eq!(package_from_cmdline(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn files_dir_follows_the_android_user() {
        assert_eq!(
            android_files_dir("dev.nuvoletta.app", 10_187),
            PathBuf::from("/data/user/0/dev.nuvoletta.app/files")
        );
        assert_eq!(
            android_files_dir("dev.nuvoletta.app", 1_010_187),
            PathBuf::from("/data/user/10/dev.nuvoletta.app/files")
        );
    }

    #[test]
    fn vault_and_settings_live_under_the_private_directory() {
        let files = android_files_dir(ANDROID_APP_ID, 10_001);
        let p = AndroidPaths::in_files_dir(&files);
        assert!(p.vault.starts_with(&files));
        assert!(p.settings.starts_with(&files));
        assert_ne!(p.vault, p.settings);
    }
}
