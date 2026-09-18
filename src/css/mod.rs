use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk4 as gtk;

pub(crate) const BASE: &str = include_str!("base.css");
pub(crate) const SQUARE_CORNERS: &str = include_str!("square_corners.css");
pub(crate) const ALBUMS: &str = include_str!("components/albums.css");
pub(crate) const PHOTO_CONTEXT_MENU: &str = include_str!("components/photo_context_menu.css");

pub(crate) mod theme_discovery;

#[cfg(target_os = "windows")]
pub(crate) const PLATFORM: &str = include_str!("windows.css");
#[cfg(target_os = "macos")]
pub(crate) const PLATFORM: &str = include_str!("macos.css");
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub(crate) const PLATFORM: &str = include_str!("linux.css");

/// Resolve a runtime resource directory such as `css/themes` or
/// `images/theme/album-covers`.
///
/// Relative paths normally resolve against the working directory: a source
/// checkout running via `cargo run`, or a packaged launcher that starts in
/// the install folder. When the folder does not exist there, fall back to
/// the same path next to the executable — `cargo build` stages a copy there
/// (see `build.rs`) and installers provide one — so the binary keeps finding
/// its resources no matter which directory it was launched from.
pub(crate) fn resolve_runtime_dir(configured: &Path) -> PathBuf {
    if configured.is_dir() {
        return configured.to_path_buf();
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(executable_dir) = executable.parent() {
            let candidate = executable_dir.join(configured);
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    configured.to_path_buf()
}

pub(crate) fn install_foundation(display: &gtk::gdk::Display) {
    let base = gtk::CssProvider::new();
    base.load_from_data(BASE);
    gtk::style_context_add_provider_for_display(
        display,
        &base,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let platform = gtk::CssProvider::new();
    platform.load_from_data(PLATFORM);
    gtk::style_context_add_provider_for_display(
        display,
        &platform,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 10,
    );

    // Always loaded; rules only match while the main window carries the
    // "square-corners" class (Settings > Library). Priority sits above the
    // theme (+1) and album (+3) providers so border-radius: 0 wins.
    let square_corners = gtk::CssProvider::new();
    square_corners.load_from_data(SQUARE_CORNERS);
    gtk::style_context_add_provider_for_display(
        display,
        &square_corners,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 4,
    );
}
