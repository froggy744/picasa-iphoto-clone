use std::collections::HashSet;
use std::rc::Rc;

use gio::prelude::*;

pub(super) fn reveal_file(file: &gio::File) -> bool {
    let Some(path) = file.path() else {
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
