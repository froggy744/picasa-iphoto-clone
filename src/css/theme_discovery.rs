//! Runtime discovery of appearance themes.
//!
//! Themes are plain folders below `css/themes` next to the executable (same
//! layout convention as the `images/` runtime folder). Each theme folder must
//! contain one stylesheet: either `theme.css` (preferred) or, failing that,
//! the only `*.css` file in the folder.
//!
//! The display name and the dark/base flags come from a metadata comment at
//! the top of the stylesheet, so adding a theme never requires a code or
//! settings change:
//!
//! ```css
//! /* picasa-theme
//!    name: Deep Ocean
//!    dark: false
//!    mode: overlay
//! */
//! ```
//!
//! `name` falls back to the folder name (humanized), `dark` defaults to
//! false, and `mode` defaults to `overlay` (`base` marks the one theme that
//! is always loaded beneath every overlay theme).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredTheme {
    /// Stable identifier: the theme folder's name. Stored in the settings
    /// database, so renaming a folder changes the user's selection.
    pub(crate) id: String,
    /// Display label from the metadata header, falling back to the folder
    /// name in humanized form.
    pub(crate) name: String,
    /// Dark themes force the app-wide color scheme to prefer dark and use
    /// the dark lightbox backdrop.
    pub(crate) dark: bool,
    /// Base themes are always loaded beneath the active overlay theme.
    pub(crate) is_base: bool,
    /// Full stylesheet text, ready for a `CssProvider`.
    pub(crate) css: String,
}

/// Scan `directory` for theme folders. A missing or unreadable directory
/// yields an empty list; the app then runs on the stock GTK appearance.
pub(crate) fn discover(directory: &Path) -> Vec<DiscoveredTheme> {
    let mut themes = Vec::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return themes,
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = match path.file_name().and_then(|name| name.to_str()) {
            Some(id) if !id.is_empty() && !id.starts_with('.') => id.to_string(),
            _ => continue,
        };
        let Some(css_path) = theme_css_in(&path) else {
            continue;
        };
        let css = match std::fs::read_to_string(&css_path) {
            Ok(css) => css,
            Err(error) => {
                eprintln!("Could not read theme {}: {error}", css_path.display());
                continue;
            }
        };
        let metadata = metadata_from_css(&css);
        themes.push(DiscoveredTheme {
            name: metadata.name.unwrap_or_else(|| humanize_id(&id)),
            id,
            dark: metadata.dark,
            is_base: metadata.is_base,
            css,
        });
    }
    themes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    themes
}

/// The theme's stylesheet: `theme.css` if present, otherwise the single
/// `*.css` file in the folder. Ambiguous folders (multiple stylesheets and
/// no `theme.css`) are rejected so a stray backup file cannot break a theme.
fn theme_css_in(directory: &Path) -> Option<PathBuf> {
    let preferred = directory.join("theme.css");
    if preferred.is_file() {
        return Some(preferred);
    }
    let mut candidates: Vec<PathBuf> = match std::fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

#[derive(Debug, Default, PartialEq)]
struct ThemeMetadata {
    name: Option<String>,
    dark: bool,
    is_base: bool,
}

/// Parse the `picasa-theme` metadata comment. The first comment block in the
/// stylesheet only counts as metadata when its first line mentions the
/// marker, so ordinary licence headers never produce theme flags.
fn metadata_from_css(css: &str) -> ThemeMetadata {
    let mut metadata = ThemeMetadata::default();
    let Some(start) = css.find("/*") else {
        return metadata;
    };
    let after_start = &css[start + 2..];
    let Some(end) = after_start.find("*/") else {
        return metadata;
    };
    let block = &after_start[..end];
    let mut lines = block.lines();
    let Some(first) = lines.next() else {
        return metadata;
    };
    if !first.to_lowercase().contains("picasa-theme") {
        return metadata;
    }
    for line in lines {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "name" if !value.is_empty() => metadata.name = Some(value.to_string()),
            "dark" => {
                metadata.dark = matches!(
                    value.to_ascii_lowercase().as_str(),
                    "true" | "yes" | "on" | "1"
                )
            }
            "mode" => metadata.is_base = value.eq_ignore_ascii_case("base"),
            _ => {}
        }
    }
    metadata
}

/// "deep_ocean" -> "Deep Ocean", "superman" -> "Superman".
fn humanize_id(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
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
    use super::*;

    fn unique_temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pic-themes-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn metadata_header_provides_name_dark_and_mode() {
        let metadata = metadata_from_css(
            "/* picasa-theme\n   name: Deep Ocean\n   dark: true\n   mode: base\n*/\n\n.window {}",
        );
        assert_eq!(metadata.name.as_deref(), Some("Deep Ocean"));
        assert!(metadata.dark);
        assert!(metadata.is_base);
    }

    #[test]
    fn metadata_requires_the_marker_on_the_first_line() {
        // An ordinary licence header must not produce theme flags.
        let metadata = metadata_from_css("/* Copyright 2024\n   name: Sneaky\n*/\n.x {}");
        assert_eq!(metadata.name, None);
        assert!(!metadata.dark);
        assert!(!metadata.is_base);
    }

    #[test]
    fn metadata_defaults_when_the_comment_or_file_has_none() {
        let bare = metadata_from_css(".window { color: red; }");
        assert_eq!(bare, ThemeMetadata::default());

        let unterminated = metadata_from_css("/* picasa-theme\n   name: No End");
        assert_eq!(unterminated, ThemeMetadata::default());

        let flagged = metadata_from_css("/* picasa-theme\n   dark: yes\n*/");
        assert!(flagged.dark);
        assert_eq!(flagged.name, None);
    }

    #[test]
    fn folder_ids_humanize_into_display_fallbacks() {
        assert_eq!(humanize_id("deep_ocean"), "Deep Ocean");
        assert_eq!(humanize_id("superman"), "Superman");
        assert_eq!(humanize_id("my--weird__name"), "My Weird Name");
    }

    #[test]
    fn discover_reads_folders_and_sorts_by_display_name() {
        let root = unique_temp_dir("discover");
        write(
            &root.join("zebra/theme.css"),
            "/* picasa-theme\n   name: Zebra\n*/\n.zebra {}",
        );
        write(
            &root.join("aqua/look.css"),
            "/* picasa-theme\n   name: Aqua\n*/\n.aqua {}",
        );
        write(&root.join("plain/plain.css"), ".plain {}");

        let themes = discover(&root);
        assert_eq!(themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), [
            "aqua", "plain", "zebra"
        ]);
        assert_eq!(themes[0].name, "Aqua");
        assert_eq!(themes[1].id, "plain");
        assert_eq!(themes[1].name, "Plain");
        assert_eq!(themes[2].name, "Zebra");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn discover_ignores_ambiguous_and_invalid_folders() {
        let root = unique_temp_dir("ambiguous");
        // Two stylesheets without theme.css: rejected.
        write(&root.join("messy/a.css"), ".a {}");
        write(&root.join("messy/b.css"), ".b {}");
        // Non-folder entries are ignored.
        write(&root.join("loose.css"), ".loose {}");
        // Dot folders are skipped.
        write(&root.join(".hidden/theme.css"), ".hidden {}");

        let themes = discover(&root);
        assert!(themes.is_empty(), "{themes:?}");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn discover_returns_empty_for_a_missing_directory() {
        assert!(discover(&unique_temp_dir("missing")).is_empty());
    }

    #[test]
    fn single_stylesheet_fallback_needs_exactly_one_css_file() {
        let root = unique_temp_dir("single-fallback");
        write(&root.join("only/style.css"), ".style {}");
        assert!(theme_css_in(&root.join("only")).is_some());
        assert_eq!(
            theme_css_in(&root.join("only")).unwrap(),
            root.join("only/style.css")
        );

        // theme.css always wins over other stylesheets.
        write(&root.join("both/theme.css"), ".theme {}");
        write(&root.join("both/other.css"), ".other {}");
        assert_eq!(
            theme_css_in(&root.join("both")).unwrap(),
            root.join("both/theme.css")
        );

        std::fs::remove_dir_all(&root).unwrap();
    }
}
