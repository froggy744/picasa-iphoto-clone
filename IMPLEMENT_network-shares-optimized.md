# Task: Network Share Photo Import

Implement **Network Shares** in PIC with SMB/CIFS and NFS.

Keep changes minimal. Reuse the existing GIO source/scanner architecture. Do not create a separate scanner and do not use Linux `mount`, `/etc/fstab`, or `sudo`.

## Required UI Flow

1. Sidebar: add **NETWORK SHARES** directly below Folders, with a `+` button.
2. Clicking `+` opens **Add Network Share**.
3. User selects:

   * **SMB / CIFS**
   * **NFS**
4. Show:

   * discovered network locations
   * Server field
   * Share / Export field
5. User clicks **Connect**.
6. Explicitly mount with GIO/GVfs.
7. After successful mount, open a folder picker rooted at the connected network location.
8. User chooses the exact photo folder.
9. Store the selected folder's stable network URI in the DB.
10. Register it as an imported root and start the normal PIC scan.

Do not register anything before the user selects a folder.

---

## URI Rules

SMB/CIFS:

```text
smb://server/share/
```

NFS:

```text
nfs://server/export/
```

CIFS uses the same SMB backend. Do not create a separate CIFS implementation.

Normalize user input before constructing the URI:

* remove existing `smb://`, `nfs://`, `cifs://`
* remove unnecessary leading/trailing slashes
* URI-escape path components correctly

Never store:

```text
/run/user/.../gvfs/...
```

Store only stable URIs such as:

```text
smb://nas/Photos/2026
nfs://server/photos/2026
```

---

## Explicit Mount

Before browsing or scanning, call:

```text
gio::File::mount_enclosing_volume
```

using:

```text
gtk::MountOperation
```

Mount must be asynchronous.

Password-protected SMB shares should use the normal GTK/GVfs authentication dialog.

Handle "already mounted" as success.

Friendly errors:

```text
NotFound / NotMounted
→ Server or share not found.

PermissionDenied
→ Access denied. Check username/password.

TimedOut / HostUnreachable
→ Server did not respond. Check that it is online.
```

Do not expose raw GIO errors unless tracing is enabled.

---

## Network Discovery

Use GIO/GVfs discovery where available.

Start from:

```text
network://
```

Enumerate children asynchronously.

Discovery is optional convenience only.

Rules:

* Never block dialog opening while discovery runs.
* Give discovery a short timeout, approximately 3 seconds.
* Show results as they arrive.
* If discovery returns nothing, manual Server/Share entry still works.
* Do not implement custom mDNS, Avahi or WS-Discovery in this phase.
* Do not add Synology-specific networking code.

If a friendly device name is available from GIO, display it.

Do not assume every discovered `network://` item is already a direct SMB/NFS URI. Resolve/use the URI supplied by GIO where possible.

---

## Folder Browser

After the network location is mounted, open a GTK folder chooser with the mounted location as its initial folder.

The user must explicitly select the folder PIC should import.

Use the selected `gio::File`, then store:

```rust
source::reference(&selected_file)
```

This must resolve to a stable remote URI.

Do not convert the selected file to a temporary native GVfs mount path.

Cancel = do nothing.

---

## Database

Keep using the existing `folders` system.

For a selected network folder:

```text
folders.path = full remote URI
folders.imported_root = 1
```

If PIC already has a display-name field/helper, use it.

Otherwise make the smallest DB change necessary to preserve a friendly name.

Do not create a completely separate network-photo database.

Remote paths are detected by URI scheme.

At minimum support:

```text
smb
cifs
nfs
```

Keep existing support for other GIO URI schemes intact.

---

## Sidebar

Local folders and network shares must be displayed separately.

Example:

```text
FOLDERS
  Pictures
  Screenshots

NETWORK SHARES                 +
  Synology Photos
  Home Server
```

Any imported root whose path is a remote URI must not also appear under local Folders.

Offline share:

```text
⚠ Synology Photos
```

Cached thumbnails must remain visible.

Right-click Network Share:

```text
Open
Retry Connection
Rescan
Remove from Library
```

**Remove from Library must never delete remote files.**

---

## Offline Behaviour

PIC must remain fully usable if the server is unavailable.

Requirements:

* cached thumbnails remain visible
* existing yellow unavailable-original indicator continues working
* startup must not wait indefinitely on remote servers
* search/albums continue using indexed DB information
* Retry Connection remounts the source
* successful reconnect restores originals without re-importing

---

## Remote Availability

Do not run potentially slow remote `query_exists()` calls directly on the GTK main thread.

Remote checks must be bounded/asynchronous.

Use a `gio::Cancellable` and approximately a 4-second timeout for remote probes.

Local path checks can remain fast/native.

---

## Scanner

Do not rewrite the scanner unless required.

PIC already uses GIO for network-capable enumeration.

The selected network URI should enter the same pipeline as an imported local folder:

```text
Selected Network Folder
        ↓
Existing Scanner
        ↓
Existing Database
        ↓
Existing Thumbnail Cache
        ↓
Grid / Albums / Search / Editor
```

Only fix URI-specific assumptions if found.

---

## Debug Tracing

When `PICASA_TRACE=1`, print:

```text
NETWORK discovery_started
NETWORK discovery_result uri=...
NETWORK discovery_complete count=...

NETWORK connect_requested uri=...
NETWORK mount_started uri=...
NETWORK mount_success uri=...
NETWORK mount_failed uri=... error=...

NETWORK browse_opened root=...
NETWORK browse_selected uri=...
NETWORK browse_cancelled

NETWORK registered uri=...
NETWORK scan_queued uri=...
```

Do not print passwords or credentials.

---

## Test Checklist

* SMB manual server/share connects.
* CIFS works through the SMB backend.
* NFS export connects.
* Authentication dialog appears when required.
* Empty discovery does not prevent manual connection.
* Discovered server can be selected.
* Successful connection opens folder browser.
* User-selected URI is stored, not `/run/user/.../gvfs`.
* Selected network folder scans normally.
* Network share appears only under NETWORK SHARES.
* Disconnect server: PIC does not freeze.
* Cached thumbnails remain visible.
* Retry restores access.
* Remove from Library only changes PIC DB.

## Implementation Rule

Before editing, inspect the existing:

```text
source.rs
scanner.rs
sidebar.rs
db/*
window/dialogs.rs
window/build.rs
window/layout.rs
```

Then make the **smallest possible implementation**.

Do not redesign unrelated PIC code.

First make it compile with:

```bash
cargo check
```

Then run the existing tests.

Do not add extra features beyond this task.


---

## Mounted-visibility policy (post-audit)

Requirement: PIC must never put a network share into Nautilus merely by
starting, browsing, importing, or recovering photos.

How this is met:

- SMB: listed and read exclusively through the direct libsmbclient
  transport. PIC creates no gvfs SMB mount at all.
- gvfs mounts PIC itself creates (NFS only) are recorded as PIC-owned and
  unmounted again after 5 minutes of no NFS activity.
- PIC only ever unmounts a mount it performed itself this session. An
  "already mounted" location may be a user mount from Nautilus and is
  never recorded, never unmounted.
- The explicit "Open in file manager" action keeps its visible mount by
  design: the mount exists FOR the file browser.

### Remaining limitation: NFS

NFS has no direct transport (gvfs is the only reader), so during active
use - folder browsing in the Add dialog, an import scan, or photo reads
(lightbox / thumbnail recovery) - PIC mounts the export through gvfs, and
that mount is session-wide and therefore Nautilus-visible while it lasts.
The idle unmount removes it 5 minutes after the last NFS activity, but in
the window between an NFS operation and that unmount the share CAN appear
in Nautilus. This does not fully meet the no-visible-mount requirement
and is accepted as a known limitation; a full fix would require an NFS
client library (e.g. libnfs) so gvfs is never needed.
