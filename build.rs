//! Stage runtime resource folders next to the built binary.
//!
//! PIC resolves `themes` and `images` relative to the working directory
//! (see `resolve_runtime_dir` in `src/css/mod.rs`). Packaging scripts copy
//! those folders next to the executable, but a plain `cargo build` did not,
//! so `target/release/pic-rs` started without appearance themes or album
//! artwork. After each build this script mirrors the packaging layout by
//! copying the current `themes/` and `images/` folders into the profile
//! directory (for example `target/release`), keeping them in sync whenever
//! their contents change.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=themes");
    println!("cargo:rerun-if-changed=images");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // OUT_DIR looks like <target>/<profile>/build/<pkg>-<hash>/out, so three
    // levels up is the profile directory that holds the binaries.
    let Some(profile_dir) = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"))
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
    else {
        return;
    };

    for folder in ["themes", "images"] {
        let source = manifest_dir.join(folder);
        if source.is_dir() {
            stage_dir(&source, &profile_dir.join(folder));
        }
    }
}

/// Replace `destination` with a fresh copy of `source`.
fn stage_dir(source: &Path, destination: &Path) {
    match fs::remove_dir_all(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            eprintln!(
                "build.rs: could not clear {}: {error}",
                destination.display()
            );
            return;
        }
    }
    if let Err(error) = copy_tree(source, destination) {
        eprintln!(
            "build.rs: could not stage {} -> {}: {error}",
            source.display(),
            destination.display()
        );
    }
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
