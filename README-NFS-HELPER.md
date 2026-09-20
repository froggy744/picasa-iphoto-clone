# PIC private NFSv4 helper — source-based implementation

This patch is based on the supplied `nfs-error.zip` snapshot, **not a live checkout of GitHub**.
It does not add Nautilus, GVfs, FUSE, or a kernel NFS mount. PIC itself remains UID 1000.
The separate C helper is linked to libnfs and receives only CAP_NET_BIND_SERVICE.
The helper has a root-owned allowlist and implements `stat`, `list_dir` and `read_file`.
It maintains one userspace NFSv4 connection per current server, mounted at the v4 pseudo-root `/`.

## 1 — Work safely in a branch

In the PIC repository, BEFORE extracting this zip:

```sh
cd ~/Downloads/picasa/picasa-iphoto-clone
git status --short
git switch -c nfs-private-helper
```

If `git status --short` shows changes, save them first: do not overwrite modified files.
Extract the zip into that repository so `src/nfs_transport.rs`, `src/source.rs`,
`src/window/build.rs` and `helper/*` have their normal locations.

## 2 — Install the *separate* helper on Fedora

```sh
sudo dnf install gcc libnfs-devel libcap pkgconf-pkg-config
bash helper/install-fedora.sh
getcap /usr/local/libexec/pic-nfs-helper
cat /etc/pic-nfs-helper.conf
```

The installed executable must be `root:root`, *not* user-writable, and show
`cap_net_bind_service=ep` in `getcap`. The config must be root-owned and not
writable by group/others. The installer preserves an existing config file.
The example authorizes ONLY `10.0.0.1 /mnt/4TBP`. To enable 4TBS or 4TBM,
edit `/etc/pic-nfs-helper.conf` as root and add their explicit paths.
Restart PIC after modifying this file so its helper reloads the policy.

Do NOT setcap `target/debug/pic-rs` or run the GTK application with sudo.
Do NOT weaken DietPi exports, file permissions, or share access.

## 3 — Live check independently of PIC (must succeed before importing)

```sh
python3 helper/smoke.py 10.0.0.1 \
  '/mnt/4TBP/Other/Tat Sing/20190917_184453.jpg'
```

Expected: LIST OK, STAT OK, READ OK, JPEG SIGNATURE OK and
PRIVATE HELPER LIVE TEST PASSED. This reads the entire real JPEG from NFS,
not a cache. If this fails, stop and inspect the precise error.

## 4 — Build & test PIC on Fedora

```sh
cargo fmt
cargo check
PIC_NFS_LIVE_FILE='nfs://DietPi.local/mnt/4TBP/Other/Tat%20Sing/20190917_184453.jpg' \
 cargo test nfs_transport::tests::live_exact_jpeg_read -- --ignored --exact --nocapture
PICASA_TRACE=1 cargo run 2>&1 | tee nfs-helper-first-run.log
```

The live test **requires** opening the JPEG and checking its signature; no
skip-on-read-error logic remains. Test NFS import and fresh offline thumbnails
in the GUI before calling the feature finished. SMB code is unchanged.

## 5 — Revert safely if needed

```sh
sudo setcap -r /usr/local/libexec/pic-nfs-helper
# In the PIC repository, only when you want to discard THIS patch's files:
# git restore src/nfs_transport.rs src/source.rs src/window/build.rs
```

Do not remove the whole application or its SQLite photo library. This patch
has not been compiled with your Fedora GTK stack or exercised against your
DietPi server from the build environment; the C helper was syntax-checked
against a libnfs API stub and its protocol was mock-tested for stat/list/read
and allowlist denial. Actual deployment and live validation remain required.
