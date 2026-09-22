# Portable Direct NFS Design

## Goal

PIC must browse NFS shares from an ordinary-user AppImage or Flatpak without a separate helper installation, root permissions, Linux capabilities, GVfs, or kernel mounts.

## Constraints

- SMB access remains on PIC's existing direct libsmbclient path.
- NFS discovery continues to use direct libnfs export discovery.
- NFS directory listing, stat, read, and range-read operations use direct libnfs in the application process.
- The standalone probe uses the same direct unprivileged libnfs operations.
- AppImage and Flatpak package the required libnfs runtime and do not look for `/usr/local/libexec/pic-nfs-helper`.
- A NAS that requires reserved source ports is reported as incompatible with an actionable message.
- No NAS configuration is changed automatically.

## Transport decision

Linux userspace cannot make a guaranteed reserved-port NFS connection from an ordinary AppImage or Flatpak without a capability or privileged helper. The supported server-side compatibility change is to add `insecure` to each intended read-only NFS export. This permits non-reserved client source ports. It weakens the server's source-port trust check, so export authentication, host restrictions, read-only mode, and any available NFS security flavor remain important.

The NAS configuration recommendation is therefore:

```text
/mnt/dietpi_userdata  *(ro,insecure)
/mnt/4TBP             10.0.0.0/24(ro,insecure)
/mnt/4TBS             10.0.0.0/24(ro,insecure)
/mnt/4TBM             10.0.0.0/24(ro,insecure)
```

The exact existing export options must be preserved and `insecure` appended rather than replacing the NAS administrator's current security settings.

## Application design

`src/network_shares.rs` calls `private_nfs` for all non-virtual-root NFS operations. `src/private_nfs.rs` remains the single direct libnfs implementation for list, stat, full read, and range read. The helper-backed transport is removed from the application module graph. Direct libnfs errors are classified where possible and include the server-side `insecure` recommendation when the server closes a connection or rejects the mount.

## Packaging design

Linux packaging bundles the direct application binary's libnfs dependency in the existing package layout. AppImage and Flatpak packaging checks assert that no helper executable, helper path, file capability, or `/usr/local` dependency is required. The portable C helper may be removed if no package target still uses it; the application itself must not spawn it.

## Diagnostics and verification

`scripts/test-shares.sh` remains the main command. Its NFS probe directly performs export discovery, NFSv3/NFSv4 mount/list operations, and file stat/read operations using libnfs. Tests cover both protocol versions, path handling, actionable incompatible-server errors, SMB discovery, Rust tests, release builds, and package dependency inspection.
