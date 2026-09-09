use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderKind {
    TurboJpeg,
    Image,
    Heif,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageFormat {
    pub id: &'static str,
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub decoder: DecoderKind,
}

pub const FORMATS: &[ImageFormat] = &[
    ImageFormat {
        id: "jpeg",
        name: "JPEG",
        extensions: &["jpg", "jpeg"],
        decoder: DecoderKind::TurboJpeg,
    },
    ImageFormat {
        id: "png",
        name: "PNG",
        extensions: &["png"],
        decoder: DecoderKind::Image,
    },
    ImageFormat {
        id: "webp",
        name: "WebP",
        extensions: &["webp"],
        decoder: DecoderKind::Image,
    },
    ImageFormat {
        id: "gif",
        name: "GIF",
        extensions: &["gif"],
        decoder: DecoderKind::Image,
    },
    ImageFormat {
        id: "bmp",
        name: "BMP",
        extensions: &["bmp"],
        decoder: DecoderKind::Image,
    },
    ImageFormat {
        id: "tiff",
        name: "TIFF",
        extensions: &["tif", "tiff"],
        decoder: DecoderKind::Image,
    },
    ImageFormat {
        id: "heif",
        name: "HEIC / HEIF",
        extensions: &["heic", "heif"],
        decoder: DecoderKind::Heif,
    },
    ImageFormat {
        id: "avif",
        name: "AVIF",
        extensions: &["avif"],
        decoder: DecoderKind::Image,
    },
    ImageFormat {
        id: "nikon_raw",
        name: "Nikon RAW",
        extensions: &["nef", "nrw"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "canon_raw",
        name: "Canon RAW",
        extensions: &["cr2", "cr3"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "sony_raw",
        name: "Sony RAW",
        extensions: &["arw"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "dng",
        name: "Digital Negative",
        extensions: &["dng"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "fujifilm_raw",
        name: "Fujifilm RAW",
        extensions: &["raf"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "olympus_raw",
        name: "Olympus RAW",
        extensions: &["orf"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "panasonic_raw",
        name: "Panasonic RAW",
        extensions: &["rw2"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "pentax_raw",
        name: "Pentax RAW",
        extensions: &["pef"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "samsung_raw",
        name: "Samsung RAW",
        extensions: &["srw"],
        decoder: DecoderKind::Raw,
    },
    ImageFormat {
        id: "generic_raw",
        name: "Generic RAW",
        extensions: &["raw"],
        decoder: DecoderKind::Raw,
    },
];

pub fn all() -> &'static [ImageFormat] {
    FORMATS
}

pub fn for_path(path: impl AsRef<Path>) -> Option<&'static ImageFormat> {
    let extension = path.as_ref().extension()?.to_str()?;
    FORMATS.iter().find(|format| {
        format
            .extensions
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

pub fn supported(path: impl AsRef<Path>) -> bool {
    for_path(path).is_some()
}

pub fn uses(path: impl AsRef<Path>, decoder: DecoderKind) -> bool {
    for_path(path).is_some_and(|format| format.decoder == decoder)
}

pub fn setting_key(format: &ImageFormat) -> String {
    format!("format-enabled-{}", format.id)
}

pub fn is_enabled(connection: &Connection, format: &ImageFormat) -> Result<bool> {
    Ok(crate::db::setting(connection, &setting_key(format))?.as_deref() != Some("false"))
}

pub fn set_enabled(connection: &Connection, format: &ImageFormat, enabled: bool) -> Result<()> {
    crate::db::set_setting(
        connection,
        &setting_key(format),
        if enabled { "true" } else { "false" },
    )
}

pub fn path_is_enabled(connection: &Connection, path: &str) -> bool {
    for_path(path).is_some_and(|format| is_enabled(connection, format).unwrap_or(true))
}

pub fn enabled_ids(connection: &Connection) -> Result<HashSet<&'static str>> {
    all()
        .iter()
        .filter_map(|format| match is_enabled(connection, format) {
            Ok(true) => Some(Ok(format.id)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub fn path_is_enabled_in(enabled: &HashSet<&str>, path: &str) -> bool {
    for_path(path).is_some_and(|format| enabled.contains(format.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_recognizes_all_declared_extensions_case_insensitively() {
        for format in all() {
            for extension in format.extensions {
                assert_eq!(for_path(format!("photo.{extension}")), Some(format));
                assert_eq!(
                    for_path(format!("photo.{}", extension.to_ascii_uppercase())),
                    Some(format)
                );
            }
        }
        assert_eq!(for_path("photo.txt"), None);
    }

    #[test]
    fn format_visibility_defaults_on_and_persists_off() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE settings (
                   key TEXT PRIMARY KEY,
                   value TEXT NOT NULL
                 );",
            )
            .unwrap();
        let heif = for_path("photo.heic").unwrap();
        assert!(is_enabled(&connection, heif).unwrap());
        set_enabled(&connection, heif, false).unwrap();
        assert!(!path_is_enabled(&connection, "photo.HEIF"));
        assert!(!crate::db::setting(&connection, &setting_key(heif))
            .unwrap()
            .is_none());
    }
}
