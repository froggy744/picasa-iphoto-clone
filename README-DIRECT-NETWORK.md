## Picasa Clone — new direct network-share system

20 Sep 2026 - 22:56

We changed Picasa Clone so it can browse photos on SMB and NFS shares directly, without requiring the user to mount the share in Linux first.

The intended experience is: add a network folder, browse its subfolders, index its photos, generate local thumbnails, and open the full-quality originals directly from the network. The originals must remain on the NAS.

This is separate from the lightbox optimization work. The direct network access works, but we still have network-performance work to finish.


## How we implemented the new network system

The new system uses direct SMB and NFS access in Picasa Clone, rather than relying on Nautilus or manually mounted folders.

The working design is:

```
Add Network Share
       ↓
Discover SMB / NFS shares
       ↓
Browse folders on DietPi
       ↓
Select a folder to import
       ↓
Store network URI in SQLite
       ↓
Scan photo metadata
       ↓
Generate local thumbnail cache
       ↓
Grid / Albums / Search / Viewer / Editor
       ↓
Read original directly from NAS when needed
```

For example, a photo can retain a reference such as:

```
nfs://DietPi.local/mnt/4TBS/Spar%20Ladies%202025/10km/DSC_6656.jpg
```

The important design rule: originals remain on DietPi. We cache thumbnails and library metadata locally, but do not download the entire photo collection or create persistent original-image copies under `~/.cache/picasa-rs/source/`.

### What we have already achieved

|
Component

|

Status

|
| --- | --- |
|

Direct SMB access through `libsmbclient`

|

Implemented

|
|

Direct NFS access through the private NFS client

|

Implemented

|
|

NFS export and subfolder discovery

|

Working

|
|

Network folder registration in the sidebar

|

Working

|
|

SQLite library references to network originals

|

Working

|
|

Local thumbnail caching

|

Working for supported formats

|
|

Opening full-resolution photos directly from NAS

|

Working

|
|

Blurry thumbnail flash in lightbox

|

Fixed in Preview 4

|
|

Fast lightbox navigation

|

Still being optimized

|

### What we still need to finish

1. Reuse network connections. The logs repeatedly show `PIC_NFS_CONNECT start` and `ok` for individual photo reads. Earlier we had made progress on per-thread NFS session reuse, but the current viewer logs still show connection activity associated with each read. We need to check whether the sessions are actually being reused across operations.

picasa-preview6.log

2. Eliminate duplicate reads. The viewer sometimes fetches the same JPEG again while moving forward and backward, including overlapping foreground and prefetch requests. The next optimization needs to coordinate in-flight image requests and improve effective RAM-cache reuse.

picasa-preview6.log

3. Improve initial network thumbnail generation. Large folders, particularly those containing Nikon NEF/RAW files, still need more efficient scanning and thumbnail generation. We should avoid unnecessary repeated full-file reads and prioritize visible thumbnails.

4. Finish network-share verification. The NFS exports `/mnt/4TBP` and `/mnt/4TBM` previously had permission/access issues that still needed checking. We also need to preserve reliable offline behavior, folder navigation, cancellation, and reconnection.

5. Keep the normal photo pipeline. The network transport should supply image data to the existing scanner, thumbnails, viewer, and editor—not create a second, separate network-only photo-processing system.

Current checkpoint: `rc4` is our saved baseline, and `viewer-optimization` is the development branch. Preview 6 is an experiment in viewer request priority, not a completed network-performance fix.

