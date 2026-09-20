# PIC NFS portable packaging source (20 September 2026)

## What is included

- `src/nfs_transport.rs`: full cross-platform replacement for PIC's NFS transport.
  It locates a packaged helper relative to the running PIC executable, including
  `.app/Contents/Helpers`, Windows `libexec`, Linux `libexec`, and AppImage `usr/libexec`.
  It keeps the existing wire protocol and `stat/list_dir/read_file` signatures,
  so it does not modify SMB, Network Shares, the scanner, or the SQLite library.
- `helper/pic-nfs-helper-portable.c`: *ordinary-user*, read-only NFSv4
  helper using libnfs; no POSIX `fork`, Linux `/etc` path, or Unix permissions
  are used by the portable runtime. Windows uses binary pipes, never CRLF text
  mode. Its wire protocol is identical to the existing Linux helper.
- `helper/CMakeLists.txt` and `scripts/package-nfs-*.sh`: developer/release
  steps for Linux, macOS, and MSYS2 UCRT64 Windows. All place the helper and
  libnfs runtime beside PIC, not in an end user's system path.
- `helper/pic-nfs-helper-secure-linux.c` and `scripts/install-secure-nfs-linux.sh`:
  optional existing Linux privileged helper for exports that reject ports
  >=1024; administrator-controlled, root-owned server/export allowlist.
- `tests`: compilable C libnfs mock with stat/list/read, binary framing,
  invalid-path and connection-failure protocol checks.

## Critical security and deployment boundary

A *bundled unprivileged helper cannot access every NFS export*. Many NFSv4
servers default to `secure`, requiring a reserved **source port** below 1024.
Unprivileged GUI clients cannot universally obtain this right on Linux/macOS,
and a cross-platform binary cannot override a server access policy.
For this case preserve the already-working, administrator-installed secure
helper on Linux. On other OSes, the user can use an OS-approved NFS mount/client
with appropriate rights; making PIC access all secure exports natively from a
single ordinary-user binary remains **not solved by this release**.

No installer should silently change exports to `insecure`, use `no_root_squash`,
setuid the GUI, or change a user's NFS server ACLs.

## Apply to your current PIC checkout

Back up your current `src/nfs_transport.rs` (you have uncommitted changes to
that file). This package is based on the source snapshot supplied in this
conversation, **not on your live modified Fedora checkout**. Review the diff
before replacing a file with local edits:

```sh
cd ~/Downloads/picasa/picasa-iphoto-clone
cp src/nfs_transport.rs /tmp/pic-nfs-transport-before-portable.rs
# Extract the ZIP into this repo, preserving its paths.
# Review: diff -u /tmp/pic-nfs-transport-before-portable.rs src/nfs_transport.rs
```

The release source does not overwrite your working `source.rs`,
`src/window/build.rs`, `src/window/dialogs.rs` or any SMB code. Your local
thumbnail performance and sidebar fixes remain their own changes.

## Linux release/build

Build packages on a suitable architecture and oldest supported libc baseline:

- Fedora: `sudo dnf install gcc pkgconf-pkg-config libnfs-devel`
- Debian/Ubuntu: `sudo apt install build-essential pkg-config libnfs-dev`
- Arch/CachyOS: `sudo pacman -S --needed base-devel pkgconf libnfs`

Then from the PIC repo:

```sh
cargo build --release
bash scripts/package-nfs-linux.sh target/release
# target/release/pic-rs and target/release/libexec/* ship together.
```

For a debug build:

```sh
bash scripts/package-nfs-linux.sh target/debug
PICASA_TRACE=1 cargo run
```

`.deb`: install PIC as `/usr/bin/pic-rs` and helper plus its libnfs shared
object as `/usr/libexec/`; `/usr/bin` -> `/usr/libexec` is located at runtime.
Arch/Fedora native packages: same paths. AppImage: `usr/bin/pic-rs` and
`usr/libexec/{pic-nfs-helper,libnfs.so.*}` within AppDir. Flatpak: `/app/bin`
and `/app/libexec`. Package and license all transitive runtime dependencies
that the target system cannot be assumed to supply, and run `ldd` in a clean
system or equivalent CI image; a desktop binary alone is not a full package.

For exports requiring privileged ports: preserve existing
`/usr/local/libexec/pic-nfs-helper` and its existing root-owned
`/etc/pic-nfs-helper.conf` from the earlier working installation. The new
transport deliberately looks for it first on Linux. A clean machine may use
`sudo bash scripts/install-secure-nfs-linux.sh` only after an administrator
has created/reviewed the per-host IP and export-prefix allowlist. Do **not**
run that script just to install a normal PIC desktop release.

## Windows UCRT64 release/build

In MSYS2 UCRT64 (same environment used to build PIC):

```sh
pacman -S --needed mingw-w64-ucrt-x86_64-gcc \
  mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-libnfs
cargo build --release --target x86_64-pc-windows-gnu
bash scripts/package-nfs-windows-ucrt64.sh \
  target/x86_64-pc-windows-gnu/release
```

Ship `pic-rs.exe`, `libexec/pic-nfs-helper.exe`, libnfs and its DLL runtime
in one application installer/portable directory. Do not require the user to
install MSYS2. A Windows build and a clean-Windows smoke test are mandatory
before a public Windows NFS claim. Windows is not granted Linux capabilities.

## macOS release/build

On each target architecture (Apple Silicon and Intel separately or universal):

```sh
brew install libnfs pkg-config
cargo build --release
bash scripts/package-nfs-macos.sh path/to/PIC.app
# Copies helper into PIC.app/Contents/Helpers.
```

Copy/rewrite any other non-system dylibs reported by `otool -L`, then
sign and notarize the **completed** app bundle. Do not assume a Homebrew
runtime exists on the user's Mac. A native macOS build/clean Mac smoke test
is mandatory before a public macOS NFS claim.

## Smoke test

```sh
python3 tests/test_protocol.py
cargo check
cargo test nfs_transport::tests -- --nocapture
PIC_NFS_LIVE_FILE='nfs://HOST/exports/photo.jpg' \
  cargo test nfs_transport::tests::live_exact_jpeg_read -- --ignored --exact --nocapture
```

`tests/test_protocol.py` checks binary protocol and safety against a mock,
not a live server. The ignored Rust test must be run on an actual allowed NFS
export on each target OS. Verify a clean first import, full restart,
thumbnail cache reuse, and server-offline behaviour. `.NEF` still transfers
whole originals through the current `R` protocol; **this portability change
is not the promised RAW-ranged-I/O optimization**.

## Licensing and release gate

PIC's small C helper is MIT-licensed here; libnfs has its own upstream license
choices and redistribution obligations. A release must bundle its license
notices and any license-required source/offers as appropriate. This repository
contains NO compiled OS binaries or upstream libnfs library source.

Validation performed while producing this source: `cc -Werror` against the
included libnfs mock; protocol test passed; packaging shell scripts passed
`bash -n`. Actual libnfs builds, PIC `cargo check`, live NFS, native Windows
and macOS distribution tests cannot be run in the present environment.
This is a **portable packaging implementation/source**, not a certification
that every platform/export combination already works in production.
