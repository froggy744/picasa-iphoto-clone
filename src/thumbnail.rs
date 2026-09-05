use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use anyhow::{Context, Result};
use fast_image_resize::{
    images::Image, pixels::PixelType, FilterType, ResizeAlg, ResizeOptions, Resizer,
};
use image::codecs::jpeg::JpegEncoder;
use image::ImageEncoder;
use image::{ColorType, DynamicImage, ImageReader};
use turbojpeg::{Decompressor, Image as TurboImage, PixelFormat, ScalingFactor};

const THUMBNAIL_SIZE: u32 = 320;
const THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v4-heif-orientation";
const RAW_THUMBNAIL_CACHE_VERSION: &[u8] = b"picasa-thumb-v5-raw-preview";

macro_rules! thumb_trace {
    ($($arg:tt)*) => {
        if std::env::var_os("PICASA_TRACE").is_some() {
            eprintln!($($arg)*);
        }
    };
}

// A folder import, startup recovery, and a manual refresh can overlap their
// thumbnail passes. Keep cache-key ownership separate from the filesystem
// existence check so two workers cannot generate the same preview together.
static IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

// Structural split only: included files remain in this module scope.
include!("thumbnail/cache.rs");
include!("thumbnail/viewer.rs");
include!("thumbnail/nef.rs");
include!("thumbnail/decoders.rs");
include!("thumbnail/batch.rs");
