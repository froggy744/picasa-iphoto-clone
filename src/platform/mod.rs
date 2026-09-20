use std::collections::HashSet;
use std::rc::Rc;

use gio::prelude::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
mod linux;
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
use linux as imp;

/// Reveal a file in the host file manager. Each platform first tries its
/// selection-capable native command and then falls back to opening the parent.
pub(crate) fn reveal_file(file: &gio::File) {
    if imp::reveal_file(file) {
        return;
    }
    if let Some(parent) = file.parent() {
        let _ = gio::AppInfo::launch_default_for_uri(&parent.uri(), None::<&gio::AppLaunchContext>);
    }
}

/// Additional mount roots that are not exposed by GIO's cross-platform
/// VolumeMonitor. Linux supplements it with UnixMountEntry; other platforms
/// need no extra probe here.
pub(crate) fn extra_mounted_roots() -> HashSet<String> {
    imp::extra_mounted_roots()
}

/// Install platform-only mount-change notifications and retain any native
/// monitor object on the window for the lifetime of the application.
pub(crate) fn install_native_mount_monitor(
    window: &libadwaita::ApplicationWindow,
    refresh: Rc<dyn Fn()>,
) {
    imp::install_native_mount_monitor(window, refresh);
}
