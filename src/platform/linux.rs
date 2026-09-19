use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use gio::prelude::*;
use glib::Variant;

/// Reveal a photograph on Linux. Prefers the standard
/// `org.freedesktop.FileManager1.ShowItems` D-Bus method, which opens the
/// containing folder with the photo selected and understands remote URIs
/// (`smb://`, `nfs://`, `sftp://`, ...) without requiring a local/gvfs path.
///
/// Everything runs asynchronously on the GTK main context - the D-Bus proxy
/// and the `ShowItems` call use the async GIO pipeline, never the blocking
/// `launch_default_for_uri` sync variant that previously stalled the UI for
/// seconds on unmounted network shares. When the service is unavailable or
/// fails, the containing directory is opened through GIO, also asynchronously.
/// Returns `true` because the fallback is owned by this chain.
pub(super) fn reveal_reference(
    target_uri: &str,
    parent_uri: Option<&str>,
    started: Instant,
) -> bool {
    let target_uri = target_uri.to_string();
    let parent_uri = parent_uri.map(str::to_owned);
    crate::source::net_trace(format!("reveal_filemanager1_start uri={target_uri}"));
    glib::spawn_future_local(async move {
        let dbus_started = Instant::now();
        match show_items_file_manager1(&target_uri).await {
            Ok(()) => crate::source::net_trace(format!(
                "reveal_filemanager1_done uri={target_uri} ms={:.1} total={:.1}",
                dbus_started.elapsed().as_secs_f64() * 1000.0,
                started.elapsed().as_secs_f64() * 1000.0,
            )),
            Err(error) => {
                crate::source::net_trace(format!(
                    "reveal_filemanager1_failed uri={target_uri} error={error} ms={:.1}",
                    dbus_started.elapsed().as_secs_f64() * 1000.0,
                ));
                super::open_parent_directory(&target_uri, parent_uri.as_deref(), started);
            }
        }
    });
    true
}

/// Build the `ShowItems(as URIs, s startup-id)` parameter tuple for one
/// target URI. Kept separate so the exact D-Bus contract is unit-tested.
fn show_items_parameters(target_uri: &str) -> Variant {
    let items = Variant::array_from_iter::<String>([target_uri.to_variant()]);
    Variant::tuple_from_iter([items, "".to_variant()])
}

/// Call `org.freedesktop.FileManager1.ShowItems` on the session bus with the
/// target URI and an empty startup id. The canonical network URI is passed
/// through unchanged so the file manager resolves/mounts it itself instead of
/// receiving a stale gvfs or materialized thumbnail path.
async fn show_items_file_manager1(target_uri: &str) -> Result<(), String> {
    let proxy = gio::DBusProxy::for_bus_future(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None,
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
    )
    .await
    .map_err(|error| format!("unavailable: {error}"))?;
    let parameters = show_items_parameters(target_uri);
    proxy
        .call_future("ShowItems", Some(&parameters), gio::DBusCallFlags::NONE, -1)
        .await
        .map_err(|error| format!("ShowItems: {error}"))?;
    Ok(())
}

pub(super) fn extra_mounted_roots() -> HashSet<String> {
    gio::UnixMountEntry::mounts()
        .0
        .into_iter()
        .map(|mount| gio::File::for_path(mount.mount_path()).uri().to_string())
        .collect()
}

pub(super) fn install_native_mount_monitor(
    window: &libadwaita::ApplicationWindow,
    refresh: Rc<dyn Fn()>,
) {
    let monitor = gio::UnixMountMonitor::get();
    let refresh_mountpoints = refresh.clone();
    monitor.connect_mountpoints_changed(move |_| refresh_mountpoints());
    monitor.connect_mounts_changed(move |_| refresh());
    unsafe {
        window.set_data("picasa-unix-mount-monitor", monitor);
    }
}

#[cfg(test)]
mod tests {
    use glib::prelude::FromVariant;

    use super::show_items_parameters;

    #[test]
    fn show_items_parameters_match_the_filemanager1_signature() {
        let uri = "smb://dietpi.local/4tbp/Work/SuperHero/Pow.png";
        let parameters = show_items_parameters(uri);
        // "(ass)" is the canonical spelling of the "(as,s)" tuple type.
        assert_eq!(parameters.type_().as_str(), "(ass)");
        assert_eq!(parameters.n_children(), 2);
        assert_eq!(
            Vec::<String>::from_variant(&parameters.child_value(0)),
            Some(vec![uri.to_string()])
        );
        assert_eq!(
            String::from_variant(&parameters.child_value(1)),
            Some(String::new())
        );
    }

    #[test]
    fn show_items_parameters_accept_local_file_uris() {
        let parameters = show_items_parameters("file:///home/peet/Pictures/a.jpg");
        assert_eq!(parameters.type_().as_str(), "(ass)");
        assert_eq!(
            Vec::<String>::from_variant(&parameters.child_value(0)),
            Some(vec!["file:///home/peet/Pictures/a.jpg".to_string()])
        );
    }
}
