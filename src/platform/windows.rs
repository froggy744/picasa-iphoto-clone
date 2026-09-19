use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use gio::prelude::*;

pub(super) fn reveal_reference(
    target_uri: &str,
    _parent_uri: Option<&str>,
    _started: Instant,
) -> bool {
    let Some(path) = gio::File::for_uri(target_uri).path() else {
        return false;
    };
    std::process::Command::new("explorer.exe")
        .arg("/select,")
        .arg(path)
        .spawn()
        .is_ok()
}

pub(super) fn extra_mounted_roots() -> HashSet<String> {
    HashSet::new()
}

pub(super) fn install_native_mount_monitor(
    _window: &libadwaita::ApplicationWindow,
    _refresh: Rc<dyn Fn()>,
) {
}
