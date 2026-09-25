use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use gtk::prelude::*;
use gtk4 as gtk;

pub struct ThemeManager {
    display: gtk::gdk::Display,
    active: RefCell<Option<gtk::CssProvider>>,
}

impl ThemeManager {
    pub fn new() -> Option<Self> {
        Some(Self {
            display: gtk::gdk::Display::default()?,
            active: RefCell::new(None),
        })
    }

    pub fn available_themes() -> Vec<String> {
        let root = themes_dir();
        let Ok(entries) = fs::read_dir(root) else {
            return vec!["standard".to_string()];
        };

        let mut names = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().join("theme.css").is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        if !names.iter().any(|name| name == "standard") {
            names.insert(0, "standard".to_string());
        }
        names
    }

    pub fn apply(&self, name: &str) -> Result<()> {
        let path = themes_dir().join(name).join("theme.css");
        let provider = gtk::CssProvider::new();

        if path.is_file() {
            provider.load_from_path(&path);
        } else if name == "standard" {
            provider.load_from_data(include_str!("../themes/standard/theme.css"));
        } else {
            anyhow::bail!("theme not found: {name}");
        }

        if let Some(previous) = self.active.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&self.display, &previous);
        }
        gtk::style_context_add_provider_for_display(
            &self.display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        self.active.replace(Some(provider));
        Ok(())
    }
}

fn themes_dir() -> PathBuf {
    let configured = Path::new("themes");
    if configured.is_dir() {
        return configured.to_path_buf();
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let candidate = directory.join("themes");
            if candidate.is_dir() {
                return candidate;
            }
        }
    }

    PathBuf::from("themes")
}

pub fn pretty_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::pretty_name;

    #[test]
    fn theme_names_are_presentable() {
        assert_eq!(pretty_name("bright-studio"), "Bright Studio");
        assert_eq!(pretty_name("standard"), "Standard");
    }
}
