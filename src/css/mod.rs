use gtk::prelude::*;
use gtk4 as gtk;

pub(crate) const BASE: &str = include_str!("base.css");
pub(crate) const ALBUMS: &str = include_str!("components/albums.css");
pub(crate) const PHOTO_CONTEXT_MENU: &str = include_str!("components/photo_context_menu.css");

pub(crate) mod themes {
    pub(crate) const STANDARD: &str = include_str!("themes/standard.css");
    pub(crate) const IPHONE: &str = include_str!("themes/iphone.css");
    pub(crate) const TEAL: &str = include_str!("themes/teal.css");
    pub(crate) const BLUE: &str = include_str!("themes/blue.css");
    pub(crate) const GLASS: &str = include_str!("themes/glass.css");
    pub(crate) const AQUA: &str = include_str!("themes/aqua.css");
    pub(crate) const SUPERMAN: &str = include_str!("themes/superman.css");
}

#[cfg(target_os = "windows")]
pub(crate) const PLATFORM: &str = include_str!("windows.css");
#[cfg(target_os = "macos")]
pub(crate) const PLATFORM: &str = include_str!("macos.css");
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub(crate) const PLATFORM: &str = include_str!("linux.css");

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
}
