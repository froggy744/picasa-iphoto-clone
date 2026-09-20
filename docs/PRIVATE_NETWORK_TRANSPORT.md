# Private network transport audit

PIC must never create a desktop or kernel mount for a registered network share.
The attached `src-smbnfs.zip` was inspected; its `source.rs` matched the checkout
before this change.

## Root cause and removed paths

GVfs mounts are session-wide. Tracking ownership and unmounting after five
minutes cannot prevent their initial appearance in Files. Direct SMB existed,
but callers could bypass it, and the direct directory decoder discarded
`SMBC_FILE_SHARE` entries, making server enumeration falsely appear empty.

| Original path | Problem | Replacement |
| --- | --- | --- |
| `source::mount_share_run` and interactive/silent/visible wrappers | `find_enclosing_mount` reused desktop mounts; `mount_enclosing_volume` created them | Removed |
| `source::ensure_nfs_mounted_blocking`, called by `read` | Mounted NFS on `NotMounted` | Explicit private-transport-unavailable error |
| `source::nfs_idle_unmount_tick/pass`, `unmount_blocking`, ownership tracking | Kept shares visible until idle; modified desktop mount state | Removed, including the window timer |
| `source::read`, `query_exists` | SMB used GIO without libsmbclient; NFS reused mounted exports | Direct SMB or unavailable |
| `scanner::root_is_available`, `scan_with_control` | GIO probing/enumeration fallback without libsmbclient | Private SMB dispatch; NFS fails before reconciliation |
| Add Share, `open_gvfs_browser_for_connect` | Empty/failed anonymous enumeration and missing libsmbclient entered GVfs | PIC authentication and private browsing |
| Network browser enumeration and mouse/keyboard navigation | Empty direct listings retried GIO; navigation could mount | Direct SMB listing only |
| Retry Connection and startup SMB probe | Interactive mount or GIO fallback | Worker-thread direct SMB |
| Share Open and “Open Network Folder in Files” | Explicit mount and/or network-URI launch | Open inside PIC; Files action removed |
| Photo reveal, Open With, rename, Trash | Desktop URI handoff or reuse of GIO network backend | Guarded; export a local copy for external apps |
| Folder monitors | GIO monitoring of registered remote roots | Network roots excluded; private availability refresh retained |
| Legacy GVfs paths, `file://` wrappers, mixed-case/CIFS aliases | Could bypass SMB dispatch and access desktop FUSE paths | Normalize before transport selection |

No `Volume::mount`, mount subprocess, or shutdown unmount path was found.
The remaining GIO network use is read-only `network://` service discovery and
shortcut resolution, not SMB/NFS file access. Live mount-list checks cover
discovery as well. `showmount -e` only lists advertised NFS exports; it does not
mount or make NFS file access available.

## Behavior

SMB browsing, metadata, scanning, thumbnails, originals and retry use
libsmbclient exclusively. Missing libraries fail closed. Saved credentials are
tried before guest access; explicit credentials are validated on a worker and
retained in memory/Secret Service. Server credentials also support browsing its
shares. Empty share lists request PIC authentication or an explicit share path.

NFS registrations and cached thumbnails remain available for offline browsing.
Reads/scans/retry report that private NFS transport is not implemented. No mount
is created, reused or unmounted for NFS. Existing user-owned mounts are untouched.

[libnfs](https://github.com/sahlberg/libnfs) is the practical future userspace
client: directory, stat and file APIs avoid kernel/GVfs mounts. Its current API
is incompatible with earlier versions, including read signatures. A safe Rust
integration needs version-matched bindings, bounded worker calls, NFS v3/v4
export/authentication handling and live failure tests. This change deliberately
uses the requested unavailable behavior instead of introducing unvalidated FFI.

Samba entry types and directory error semantics were checked against the
[upstream header](https://github.com/samba-team/samba/blob/master/source3/include/libsmbclient.h)
and [directory implementation](https://github.com/samba-team/samba/blob/master/source3/libsmb/libsmb_dir.c).

## Verification

Results are recorded after the final validation run. The live regression test is
`smb_transport::tests::private_network_mount_list_smoke`; run it with an isolated
`XDG_CACHE_HOME` and `XDG_DATA_HOME`. It snapshots all `gio mount -l` mount URIs
before testing and checks equality after each operation, including existing
local mounts. It uses the repository's existing DietPi test share.

Interactive Add/Retry dialog operation, physical network disconnection,
authenticated-server rejection and a full GUI restart are separate manual
acceptance checks; transport smoke tests do not claim to replace them.
