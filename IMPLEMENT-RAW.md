# PIC — Remote RAW Support: Build Repair, Integration and Verification

You are working on PIC (Picasa iPhoto Clone), a Rust + GTK4/libadwaita photo manager for Linux.

Project directory:
`/home/peet/Downloads/picasa/raw-picasa`

## Context

We recently implemented native NFS and SMB network integration, improved network browsing performance, fixed cancellation behaviour, and resolved mouse-wheel navigation issues.

We then introduced remote RAW support, including bounded network range reads and extraction of embedded JPEG previews from RAW files.

The RAW implementation was previously delivered as `pic_raw_network_edited_files_only.zip`, containing 10 edited files.

The current build fails during the native C compilation stage.

Cargo reports:

```
error: failed to run custom build command for pic-rs v1.0.0

cargo:rerun-if-changed=native/private_smb.c
cargo:rerun-if-changed=native/private_nfs.c
```

The compiler output also references `S_ISDIR`.

The complete compiler error must be investigated rather than assuming the missing header is the only problem.

## Phase 1 — Investigate and repair compilation

Inspect the current source tree and Git status before making changes.

Read:

* `build.rs`
* `native/private_smb.c`
* `native/private_nfs.c`
* Relevant Rust FFI declarations and network-source modules
* The current RAW decoding implementation

Run:

```
cargo build -vv
```

Identify the exact compiler failure, including the file, line number and compiler diagnostic.

Check POSIX includes such as `<sys/stat.h>` where appropriate, but do not apply speculative fixes.

Repair the actual cause with the smallest practical change.

Do not run `cargo clean` unless there is evidence that stale build artifacts are involved.

Once repaired, run:

```
cargo build
cargo test
```

Do not proceed under the assumption that compilation succeeds. Verify it.

## Phase 2 — Inspect existing remote RAW implementation

Determine whether the earlier remote RAW implementation is actually present in this checkout.

Trace the complete path from selecting a remote RAW thumbnail to displaying its image in the lightbox.

Inspect the relevant code for:

* RAW format detection
* Network-source dispatch
* Native NFS and SMB readers
* Bounded reads and file offsets
* Nikon NEF embedded JPEG extraction
* RAW thumbnail generation
* Lightbox image decoding
* Cancellation and stale-result handling
* Temporary-file and cache behaviour

Identify any remaining code paths that call `source::materialize()` or an equivalent operation on a remote RAW original.

Do not assume that an existing materialization function is safe merely because it is already used elsewhere.

Check what it actually reads and writes.

## Phase 3 — Critical remote RAW requirements

The fundamental requirement is:

**PIC must never silently download or permanently cache a complete remote RAW original for thumbnail generation or normal lightbox viewing.**

The previous implementation caused approximately 20 GB of unnecessary downloads.

This must not happen again.

Required behaviour:

1. Opening an NFS or SMB folder should not download its photos.
2. Indexing should use metadata and bounded reads where feasible.
3. Thumbnail generation should prefer embedded RAW thumbnails or JPEG previews.
4. Lightbox viewing should retrieve only the data required for the embedded preview whenever the format supports it.
5. Network reads must be bounded and use explicit offsets.
6. Preserve the original remote URI as the source identity in SQLite.
7. Do not replace a remote URI with a temporary local file path.
8. Do not retain complete RAW originals in the thumbnail cache.
9. Do not modify, rename or write to the original files on the NAS.
10. If an embedded preview cannot be extracted without a full download, report the unsupported case or use an existing cached thumbnail rather than silently downloading the original.

The application should continue to use the existing native NFS/SMB architecture.

Do not replace it with a new network implementation or introduce an unnecessary dependency.

## Phase 4 — Preserve existing functionality

Do not regress the following recently completed work:

* Fast native NFS and SMB access
* Existing connection/session handling
* Network read cancellation
* Stale-result rejection
* Bounded concurrent image decoding
* Thumbnail caching
* JPEG and HEIF decoding
* Mouse-wheel photo navigation
* Ctrl+wheel zoom
* Lightbox 1:1 viewing
* Offline photo indicators
* Background scanning
* Existing network diagnostics

Do not change protocol timeout or retransmission behaviour merely to hide a slow operation.

If the existing code has a separate RAW-specific problem, isolate the fix to that path.

## Phase 5 — Verification

After implementing the necessary fixes, verify the following:

### Build verification

Run:

```
cargo fmt --check
cargo build
cargo test
```

Fix compilation errors introduced by your changes.

Distinguish existing warnings from new warnings.

### RAW network verification

Test an actual remote Nikon NEF file over NFS and SMB if the shares and samples are accessible.

For each protocol, verify:

* RAW thumbnail generation
* Lightbox opening
* Embedded JPEG preview extraction
* Navigation to another photo
* Cancellation of an obsolete request
* No complete RAW-original download
* No permanent RAW-original cache file
* No modification of the NAS original

Add focused automated tests for bounded reads, offset handling and extraction boundaries where practical.

Do not claim NFS/SMB integration tests passed if the actual shares were unavailable.

If real network testing is impossible, explain exactly what remains unverified.

### Diagnostic logging

Use existing `PICASA_TRACE=1` diagnostics.

Where useful, record:

* Protocol
* File extension
* Requested offset
* Requested length
* Bytes actually received
* Preview size
* Thumbnail or lightbox operation
* Total network bytes read for the operation
* Whether cancellation occurred

Avoid excessive logging during normal operation.

Never log credentials.

## Phase 6 — Final report

When finished, provide:

1. Exact root cause of the C compilation failure.
2. Files modified.
3. Confirmation that `cargo build` succeeds or the exact remaining error.
4. Number of tests passed and failed.
5. Whether remote RAW support was already present or required further implementation.
6. Whether full remote RAW downloads are still possible through normal thumbnail/lightbox paths.
7. NFS and SMB test results, clearly distinguishing actual network tests from unit tests.
8. Remaining limitations.

Do not report an implementation as complete merely because it compiles.

## Working rules

This is an existing application, not a new project.

Preserve the current architecture and working features.

Do not rewrite unrelated modules.

Do not revert recent network performance fixes.

Do not make unrelated UI changes.

Do not delete the user's existing photo library, thumbnail cache or SQLite database.

Do not run destructive Git commands.

Do not commit or push unless explicitly instructed.

Make the smallest reliable changes necessary to repair compilation and complete remote RAW support.

Proceed with the investigation, fixes and available tests without stopping for routine confirmation.

