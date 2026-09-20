use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

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

/// Reveal a local photograph in the host file manager, asynchronously.
/// Private network references (including legacy FUSE paths) stay inside PIC.
pub(crate) fn reveal_reference(reference: &str) {
    // Never hand a private network URI to a desktop service that may mount it.
    if crate::source::is_network_location(reference) {
        return;
    }
    let (target_uri, parent_uri) = reveal_target_uris(reference);
    crate::source::net_trace(format!(
        "reveal_start reference={reference} uri={target_uri} parent={}",
        parent_uri.as_deref().unwrap_or("<none>")
    ));
    let started = Instant::now();
    if !imp::reveal_reference(&target_uri, parent_uri.as_deref(), started) {
        open_parent_directory(&target_uri, parent_uri.as_deref(), started);
    }
}

/// Canonical target/parent URIs for a stored photo reference. Canonical
/// network URIs (`smb://`, `nfs://`, ...) pass through unchanged; legacy
/// GVfs-FUSE paths normalize back to their canonical `smb://` URI; plain
/// local paths and `file://` URIs become `file://` URIs. A materialized
/// thumbnail/source-cache path must never reach this function - callers pass
/// the photo's stored reference.
fn reveal_target_uris(reference: &str) -> (String, Option<String>) {
    let reference = crate::source::normalize_import_reference(reference);
    let file = crate::source::file(&reference);
    let target_uri = file.uri().to_string();
    let parent_uri = file.parent().map(|parent| parent.uri().to_string());
    (target_uri, parent_uri)
}

/// Open the containing folder (or the folder itself when the target has no
/// parent) through GIO's default handler, asynchronously so the GTK thread is
/// never blocked by a gvfs mount or a missing app handler. `started` anchors
/// the completion timing log for the whole reveal operation.
fn open_parent_directory(target_uri: &str, parent_uri: Option<&str>, started: Instant) {
    let directory = parent_uri
        .filter(|parent| !parent.is_empty())
        .unwrap_or(target_uri);
    if crate::source::is_network_location(directory) {
        return;
    }
    crate::source::net_trace(format!("reveal_fallback_start directory={directory}"));
    let fallback_started = Instant::now();
    let directory = directory.to_string();
    glib::spawn_future_local(async move {
        let result =
            gio::AppInfo::launch_default_for_uri_future(&directory, None::<&gio::AppLaunchContext>)
                .await;
        match result {
            Ok(()) => crate::source::net_trace(format!(
                "reveal_fallback_done directory={directory} ms={:.1} total={:.1}",
                fallback_started.elapsed().as_secs_f64() * 1000.0,
                started.elapsed().as_secs_f64() * 1000.0,
            )),
            Err(error) => crate::source::net_trace(format!(
                "reveal_fallback_failed directory={directory} error={error} ms={:.1} total={:.1}",
                fallback_started.elapsed().as_secs_f64() * 1000.0,
                started.elapsed().as_secs_f64() * 1000.0,
            )),
        }
    });
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

#[cfg(test)]
mod tests {
    use super::reveal_target_uris;

    #[test]
    fn canonical_smb_uri_is_used_unchanged() {
        let (target, parent) = reveal_target_uris("smb://dietpi.local/4tbp/Work/SuperHero/Pow.png");
        assert_eq!(target, "smb://dietpi.local/4tbp/Work/SuperHero/Pow.png");
        assert_eq!(
            parent.as_deref(),
            Some("smb://dietpi.local/4tbp/Work/SuperHero")
        );
    }

    #[test]
    fn legacy_gvfs_fuse_path_normalizes_to_canonical_smb_uri() {
        let (target, parent) = reveal_target_uris(
            "/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbp/Work/SuperHero/Pow.png",
        );
        assert_eq!(target, "smb://dietpi.local/4tbp/Work/SuperHero/Pow.png");
        assert_eq!(
            parent.as_deref(),
            Some("smb://dietpi.local/4tbp/Work/SuperHero")
        );
    }

    #[test]
    fn local_path_becomes_file_uri_with_file_parent() {
        let (target, parent) = reveal_target_uris("/home/peet/Pictures/a.jpg");
        assert_eq!(target, "file:///home/peet/Pictures/a.jpg");
        assert_eq!(parent.as_deref(), Some("file:///home/peet/Pictures"));
    }

    #[test]
    fn other_network_schemes_pass_through() {
        let (target, parent) = reveal_target_uris("nfs://host/export/a.png");
        assert_eq!(target, "nfs://host/export/a.png");
        assert_eq!(parent.as_deref(), Some("nfs://host/export"));
    }
}
