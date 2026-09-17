//! Appearance theme engine.
//!
//! Themes are discovered at runtime from the `css/themes` folder next to the
//! executable (see [`crate::css::theme_discovery`]); nothing about the
//! available themes is hardcoded. Layering on the display, lowest first:
//!
//! 1. Foundation (`css::install_foundation`): base + platform CSS.
//! 2. The `mode: base` theme (shipped as iPhoto Dark) at `APPLICATION`
//!    priority, added after the foundation so it wins for equal specificity.
//! 3. The selected overlay theme at `APPLICATION + 1`.
//!
//! The Themes page in Settings is the single picker; the header-bar button
//! opens it and doubles as the sun/moon appearance indicator.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use libadwaita as adw;
use rusqlite::Connection;

use super::THEME_SETTING_KEY;
use crate::css::theme_discovery::{self, DiscoveredTheme};

pub(crate) const THEMES_DIRECTORY: &str = "css/themes";

/// Fallback when the saved theme id no longer exists on disk.
const DEFAULT_THEME_ID: &str = "standard";

pub(crate) struct ThemeEngine {
    display: gdk::Display,
    style_manager: adw::StyleManager,
    lightbox: Rc<crate::lightbox::Lightbox>,
    connection: Rc<RefCell<Connection>>,
    base_provider: RefCell<Option<gtk::CssProvider>>,
    base_id: RefCell<Option<String>>,
    overlay_provider: RefCell<Option<gtk::CssProvider>>,
    active_id: RefCell<String>,
    appearance_button: RefCell<Option<gtk::Button>>,
    /// Runs after the user switches themes: redraws the albums home so its
    /// covers pick up the new palette. Not invoked during startup, when the
    /// albums home does not exist yet.
    post_apply: RefCell<Option<Rc<dyn Fn()>>>,
}

impl ThemeEngine {
    pub(crate) fn new(
        display: gdk::Display,
        connection: Rc<RefCell<Connection>>,
        lightbox: Rc<crate::lightbox::Lightbox>,
    ) -> Rc<Self> {
        let style_manager = adw::StyleManager::default();
        let engine = Rc::new(Self {
            display,
            style_manager: style_manager.clone(),
            lightbox,
            connection,
            base_provider: RefCell::new(None),
            base_id: RefCell::new(None),
            overlay_provider: RefCell::new(None),
            active_id: RefCell::new(String::new()),
            appearance_button: RefCell::new(None),
            post_apply: RefCell::new(None),
        });
        // While a light theme is active the lightbox backdrop follows the
        // system preference; dark themes own their backdrop regardless.
        {
            let engine = engine.clone();
            style_manager.connect_dark_notify(move |manager| {
                if !engine.active_is_dark() {
                    engine.lightbox.use_standard_backdrop(manager.is_dark());
                }
            });
        }
        engine
    }

    /// Rescan `css/themes`. Call whenever the picker is (re)built so themes
    /// dropped into the folder appear without a restart.
    pub(crate) fn themes(&self) -> Vec<DiscoveredTheme> {
        theme_discovery::discover(Path::new(THEMES_DIRECTORY))
    }

    pub(crate) fn active_id(&self) -> String {
        self.active_id.borrow().clone()
    }

    pub(crate) fn active_is_dark(&self) -> bool {
        self.themes()
            .iter()
            .find(|theme| theme.id == self.active_id())
            .map(|theme| theme.dark)
            .unwrap_or(false)
    }

    pub(crate) fn set_appearance_button(&self, button: &gtk::Button) {
        self.appearance_button.borrow_mut().replace(button.clone());
    }

    pub(crate) fn set_post_apply(&self, hook: Rc<dyn Fn()>) {
        self.post_apply.borrow_mut().replace(hook);
    }

    /// Apply the persisted theme before the window's first rendered frame.
    /// Loads the base theme beneath everything, then resolves the saved
    /// selection: saved id, else the default, else the first theme found.
    pub(crate) fn startup(&self) {
        let themes = self.themes();

        if let Some(base) = themes.iter().find(|theme| theme.is_base) {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(base.css.as_str());
            gtk::style_context_add_provider_for_display(
                &self.display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            self.base_provider.borrow_mut().replace(provider);
            self.base_id.borrow_mut().replace(base.id.clone());
        }

        let saved = crate::db::setting(&self.connection.borrow(), THEME_SETTING_KEY)
            .ok()
            .flatten()
            .unwrap_or_else(|| DEFAULT_THEME_ID.to_string());
        let active = themes
            .iter()
            .find(|theme| theme.id == saved)
            .or_else(|| themes.iter().find(|theme| theme.id == DEFAULT_THEME_ID))
            .or_else(|| themes.first())
            .cloned();
        if let Some(theme) = active {
            self.activate(&theme);
        }
    }

    /// Switch to a theme the user picked: swap the overlay, persist the
    /// choice, and refresh theme-dependent views. Skipped when the theme is
    /// already active so rebuilding the picker cannot cause redundant work.
    pub(crate) fn select(&self, theme: &DiscoveredTheme) {
        if theme.id == self.active_id() {
            return;
        }
        self.activate(theme);
        if let Err(error) =
            crate::db::set_setting(&self.connection.borrow(), THEME_SETTING_KEY, &theme.id)
        {
            eprintln!("Could not save appearance theme: {error}");
        }
        if let Some(hook) = self.post_apply.borrow().as_ref() {
            hook();
        }
    }

    fn activate(&self, theme: &DiscoveredTheme) {
        // Swap the overlay provider. Selecting the base theme itself only
        // needs the base layer that is already on the display.
        if let Some(old) = self.overlay_provider.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&self.display, &old);
        }
        let selects_base = self.base_id.borrow().as_deref() == Some(theme.id.as_str());
        if !selects_base {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(theme.css.as_str());
            gtk::style_context_add_provider_for_display(
                &self.display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            self.overlay_provider.borrow_mut().replace(provider);
        }

        *self.active_id.borrow_mut() = theme.id.clone();

        // Force the app-wide color scheme so widgets the theme CSS does not
        // reach (title bar, popovers, dialogs, the settings window) follow
        // the dark appearance too.
        if theme.dark {
            self.style_manager.set_color_scheme(adw::ColorScheme::PreferDark);
            self.lightbox.use_iphone_backdrop();
        } else {
            self.style_manager.set_color_scheme(adw::ColorScheme::Default);
            self.lightbox.use_standard_backdrop(self.style_manager.is_dark());
        }
        self.update_appearance_icon(theme.dark);
    }

    fn update_appearance_icon(&self, dark: bool) {
        if let Some(button) = self.appearance_button.borrow().as_ref() {
            button.set_icon_name(if dark {
                "weather-clear-night-symbolic"
            } else {
                "weather-clear-symbolic"
            });
        }
    }
}

/// Header-bar button. Opens Settings > Themes (wired by the caller) and its
/// sun/moon icon mirrors the active theme's `dark` flag.
pub(crate) fn appearance_button() -> gtk::Button {
    let button = gtk::Button::new();
    button.set_icon_name("weather-clear-symbolic");
    button.set_tooltip_text(Some("Appearance"));
    button
}
