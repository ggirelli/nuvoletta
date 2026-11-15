//! Nuvoletta desktop application: a thin Dioxus view over `nuvoletta-app-core`.
//!
//! All behaviour (session, autosave, auto-lock, queries) lives in `nuvoletta-app-core`; this
//! crate renders it, forwards input, and runs the JS bridge to the vendored CodeMirror editor.

mod assets;
mod bridge;
mod core;
mod dialogs;
mod editor;
mod picker;
mod platform;
mod root;
mod sidebar;
mod sync;
mod ui;

/// Brand app icon (`assets/brand/02-app-icon.svg` rendered at 256 px).
#[cfg(not(target_os = "android"))]
const ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

/// Window and webview configuration shared by desktop and mobile.
fn config() -> platform::shell::Config {
    let config = platform::shell::Config::new()
        // The CSP comes first in <head>, before anything else can load.
        .with_custom_head(assets::head())
        // Belt and braces: the webview never navigates away from the app. Links in the preview
        // are intercepted in JS and handled in Rust; anything else that tries is refused.
        .with_navigation_handler(|_| false);
    #[cfg(not(target_os = "android"))]
    let config = {
        use platform::shell::{LogicalSize, WindowBuilder, icon_from_memory, tao::window::Icon};
        let window = WindowBuilder::new()
            .with_title("Nuvoletta")
            .with_inner_size(LogicalSize::new(1180.0, 760.0))
            .with_min_inner_size(LogicalSize::new(420.0, 480.0));
        let config = config.with_window(window);
        match icon_from_memory::<Icon>(ICON_PNG) {
            Ok(icon) => config.with_icon(icon),
            Err(_) => config,
        }
    };
    config
}

fn main() {
    #[cfg(not(target_os = "android"))]
    let builder = dioxus::LaunchBuilder::desktop();
    #[cfg(target_os = "android")]
    let builder = dioxus::LaunchBuilder::mobile();
    builder.with_cfg(config()).launch(root::Root);
}
