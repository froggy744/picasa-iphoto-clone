# Direct SMB/NFS on clean Picasa main — experimental Fedora source

This package is based on the **user-supplied `picasa-main-clean.zip`**, not the
previous network-share branch. Local import, library, sidebar, grid, SQLite,
and thumbnail code remain the clean-main implementations.

## Design boundary (no original-copy import)

- SMB uses `libsmbclient`, NFS uses `libnfs` and explicit v3-then-v4 session setup.
- `network:///` is used for **discovery only**. The app never asks GIO to mount
  `smb://` or `nfs://` for network browsing, scanning, stat, or image reads.
- The Add Folder choice offers **Local folder** or **Network shares**. In the
  network picker, double-click directories, then Import selected folder.
- Store canonical remote URIs and file fingerprints in the existing SQLite
  library. Network image reads happen on demand into memory for thumbnails,
  NOT `~/.cache/picasa-rs/source/`. Existing cached thumbnail JPGs remain local.
- Indexing network files uses stat/mtime/size (not eager full image reads).
  Four workers at most decode network thumbnails; Picasa's existing
  in-flight cache guard avoids duplicate output-cache work.
- Network RAW (NEF/DNG/etc.) is **indexed metadata-only, without a thumbnail
  in this experimental build**. A path-only RAW decoder cannot be allowed to
  bulk-download files. Future implementation needs bounded/ranged embedded
  preview extraction or explicitly scoped on-demand temporary decoding.
- Previously materialized originals from OLD Picasa installations are not
  deleted automatically; review them separately.
- Write operations, opening a network source in an external desktop app, and
  GIO monitoring for network sources are not part of this direct transport.

## Build (Fedora)

```
sudo dnf install gcc pkgconf-pkg-config libsmbclient-devel libnfs-devel gtk4-devel libadwaita-devel
cargo build --release
PICASA_TRACE=1 target/release/pic-rs 2>picasa-direct-shares.log
```

Then Add Folder -> Network shares -> NFS file sharing on DietPi -> 4TBS ->
pics-sport -> choose a JPEG folder. Repeat SMB after the NFS test. Test on a
new/different library first, rather than your regular library, to avoid
mixing database changes during proof-of-concept tests.

## Current verification status

This code was inspected and packaged, but **Cargo, GTK development libraries,
Samba headers and libnfs are absent in the build environment here**. No
successful build or live DietPi test is claimed. The original clean ZIP has
not been modified. This package is for a Fedora build/test and may still need
compile/API fixes. Do not merge it to main before validation.

Known limitation: SMB auth callback is guest-only as in the successful
standalone DietPi test. No credentials manager in this phase. v0.7.4's
standalone thumbnail UI queue is NOT pasted into Picasa; Picasa's existing
bounded Rayon worker pool and cache in-flight deduplication are used instead.


## Read-session optimization (preview 2)

`pic_nfs_read` reuses one userspace NFS context per worker thread and exact host/export.
It does not retain full originals or create system/GVfs mounts. A transport error
invalidates the affected thread's session so the next read reconnects.
Watch `PIC_NFS_CONNECT reuse` versus `PIC_NFS_CONNECT start`; a fresh process or
new worker thread still needs its first connection. The NFS permission errors on
4TBP/4TBM are unchanged.

This is a transport-only improvement; viewer repeated reads or thumbnail decoding
are not changed in this package. Test builds and live DietPi measurements on Fedora.

## Preview 3: lightbox adjacent-photo prefetch

After 250 ms on a photo, the lightbox warms both immediate neighboring display
textures in its existing RAM-only, count/byte-bounded cache; the direction of
travel is scheduled first. Navigation cancels stale speculative work; the
existing viewer decode gate still bounds concurrent decodes. This does not add
a disk cache, mounts, or writes to `source/`. The current image remains visible
while an uncached next image loads. Network RAW limitations remain unchanged.

On Fedora: `cargo build --release`, then test Arrow Left/Right, fast key repeat,
close/reopen, and confirm the source cache remains empty. Full Rust/GTK and
live DietPi checks were not available in the packaging environment.

## Preview 4: no thumbnail flash in lightbox

On initial open the full-size viewer has a neutral background until a display-quality
photo is available (or uses a RAM-cached display texture). During photo navigation
it retains the previous full-quality image on cache misses and swaps directly to
the newly decoded full-quality image. The grid still uses cached thumbnails.
Direct SMB/NFS, bounded prefetch and the no-copy `source/` guard are unchanged.
