//! Per-platform glue: where files live, the folder dialog, opening links, and which Dioxus shell
//! (desktop or mobile, the same wry/tao stack) is in use. Behaviour that can be tested without
//! a device lives in `nuvoletta_app_core::platform`.

use std::path::PathBuf;

#[cfg(feature = "desktop")]
pub use dioxus::desktop as shell;
#[cfg(all(feature = "mobile", not(feature = "desktop")))]
pub use dioxus::mobile as shell;

/// True on Android: the vault is fixed in app-private storage and sync uses WebDAV only.
pub const ANDROID: bool = cfg!(target_os = "android");

/// The settings file.
#[cfg(not(target_os = "android"))]
pub fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("nuvoletta")
        .join("settings.json")
}

/// The app-private files directory (`Context.getFilesDir()`), derived without JNI from the
/// process name and uid (see `nuvoletta_app_core::android_files_dir`).
#[cfg(target_os = "android")]
fn files_dir() -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let package = std::fs::read("/proc/self/cmdline")
        .ok()
        .and_then(|c| nuvoletta_app_core::package_from_cmdline(&c))
        .unwrap_or_else(|| nuvoletta_app_core::ANDROID_APP_ID.to_owned());
    // /proc/self is owned by the process uid.
    let uid = std::fs::metadata("/proc/self").map_or(0, |m| m.uid());
    nuvoletta_app_core::android_files_dir(&package, uid)
}

/// The settings file.
#[cfg(target_os = "android")]
pub fn settings_path() -> PathBuf {
    nuvoletta_app_core::AndroidPaths::in_files_dir(&files_dir()).settings
}

/// On Android the vault always lives in app-private storage (other apps cannot read it, and it
/// is excluded from cloud backup, see `AndroidManifest`); there is nothing for the user to pick.
#[cfg(target_os = "android")]
pub fn fixed_vault_dir() -> Option<PathBuf> {
    Some(nuvoletta_app_core::AndroidPaths::in_files_dir(&files_dir()).vault)
}

/// On desktop the user chooses the vault folder.
#[cfg(not(target_os = "android"))]
pub fn fixed_vault_dir() -> Option<PathBuf> {
    None
}

/// Ask the OS for a folder; `None` if cancelled or no dialog is available.
#[cfg(not(target_os = "android"))]
pub async fn pick_folder(title: &str) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await
        .map(|h| h.path().to_path_buf())
}

/// No folder dialog on Android (see the sync assessment in `docs/ADR/0006-android-app.md`).
#[cfg(target_os = "android")]
pub async fn pick_folder(_title: &str) -> Option<PathBuf> {
    None
}

/// Open an `http`, `https` or `mailto` link in the system browser (on Android `webbrowser`
/// starts an `ACTION_VIEW` intent, which needs no permission).
pub fn open_external(url: &str) -> bool {
    webbrowser::open(url).is_ok()
}
