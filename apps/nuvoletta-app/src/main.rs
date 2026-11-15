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
mod root;
mod sidebar;
mod sync;
mod ui;

use dioxus::desktop::tao::window::Icon;
use dioxus::desktop::{Config, LogicalSize, WindowBuilder, icon_from_memory};

/// Brand app icon (`assets/brand/02-app-icon.svg` rendered at 256 px).
const ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

fn main() {
    let window = WindowBuilder::new()
        .with_title("Nuvoletta")
        .with_inner_size(LogicalSize::new(1180.0, 760.0))
        .with_min_inner_size(LogicalSize::new(420.0, 480.0));
    let mut config = Config::new()
        .with_window(window)
        // The CSP comes first in <head>, before anything else can load.
        .with_custom_head(assets::head())
        // Belt and braces: the webview never navigates away from the app. Links in the preview
        // are intercepted in JS and handled in Rust; anything else that tries is refused.
        .with_navigation_handler(|_| false);
    if let Ok(icon) = icon_from_memory::<Icon>(ICON_PNG) {
        config = config.with_icon(icon);
    }
    dioxus::LaunchBuilder::desktop()
        .with_cfg(config)
        .launch(root::Root);
}
