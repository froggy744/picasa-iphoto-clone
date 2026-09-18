# Implementation Guide — Network Shares Phase 1 (SMB)

Reproduces the working Network Shares feature on a clean tree. Standalone from
`IMPLEMENT.scroll-to-focus.md` — both guides touch `src/window/build.rs`, but in
different regions, so they can be applied in either order.

Scope (Phase 1): **SMB only**. No NFS discovery, no Windows-specific support,
no manual mounts, no sudo, no fstab, no permanent mount points. Removing a
share from PIC **never deletes files on the server**.

## Final behavior

```
NETWORK SHARES  +          ← sidebar section below Folders
        ↓ Enter server (+ optional share) → Connect
        ↓ gvfs mounts (guest = silent; password-protected = standard dialog)
        ↓ folder browser opens (share list at server root, or inside the share)
        ↓ user browses and picks the actual photo folder
        ↓ PIC registers that folder URI + scans it (worker thread)
```

- Registered shares are **persistent** (rows in the `folders` table), shown in
  the NETWORK SHARES sidebar section with photo count and an offline ⚠ badge.
- Photos appear in Library / Search / Albums like any folder; thumbnails live
  in PIC's local cache and remain visible while the server is offline.
- "Retry Connection" remounts through gvfs and re-probes availability; when the
  server returns, originals become available without re-importing.
- Startup with the NAS offline works (probes are time-bounded).
- Right-click a share: **Open / Retry Connection / Rescan / Remove from Library**.

## Why: audit result (what was already there / what blocked)

- `scanner.rs`, `thumbnail/*`, `window/availability.rs` are **already GIO-based**
  (`gio::File::enumerate_children`, `source::read`, `source::materialize`,
  availability probes). A URI root needs **no scanner changes**.
- Blockers found: gvfs **does not auto-mount** on existence checks (an
  unmounted share always reads "unavailable" — needs an explicit
  `mount_enclosing_volume`); URI probes can stall for minutes on an unreachable
  host (needs a bounded timeout, since some `db::folders()` calls run on the
  UI thread); and all the UI (section, dialog, browse flow) was missing.

---

## Step 1 — `src/source.rs`: bounded probes, mount, traces

### 1a. Add the gtk import next to the existing gio import (top of file)

```rust
use gtk4 as gtk;
```

### 1b. Replace `fn query_exists` with the bounded version + add the probe helper

```rust
fn query_exists(reference: &str, directory: bool) -> bool {
    if !reference.contains("://") {
        return if directory {
            Path::new(reference).is_dir()
        } else {
            Path::new(reference).is_file()
        };
    }
    uri_query_exists(&file(reference))
}

/// Longest a remote existence probe may block. gvfs SMB/NFS lookups can stall
/// for a very long time when a host is unreachable; without a bound, callers
/// on the GTK thread (sidebar population, startup) would freeze for minutes.
/// The probe result is cached per refresh generation, so this bounds the cost
/// of the rare uncached probe instead of recurring per row.
const URI_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

fn uri_query_exists(uri_file: &gio::File) -> bool {
    let cancellable = gio::Cancellable::new();
    let timer = cancellable.clone();
    std::thread::spawn(move || {
        std::thread::sleep(URI_PROBE_TIMEOUT);
        timer.cancel();
    });
    let exists = uri_file.query_exists(Some(&cancellable));
    // Release the timer thread early on a fast answer.
    cancellable.cancel();
    exists
}
```

### 1c. After `pub fn refresh_availability()`, add

```rust
/// Trace log for the network-share path, enabled with PIC_DEBUG_NET=1.
pub(crate) fn net_debug(message: impl std::fmt::Display) {
    if std::env::var_os("PIC_DEBUG_NET").is_some() {
        eprintln!("[net-debug] {message}");
    }
}

/// True when a URI points at a remote, gvfs-managed location (smb://, nfs://,
/// sftp://, ...) instead of a local path.
pub fn is_network_location(reference: &str) -> bool {
    let Some(scheme_end) = reference.find("://") else {
        return false;
    };
    let scheme = reference[..scheme_end].to_ascii_lowercase();
    matches!(scheme.as_str(), "smb" | "nfs" | "sftp" | "ssh" | "ftp" | "dav" | "davs" | "afc")
}

/// Mount a remote location through gvfs (never a manual mount, never fstab).
/// gvfs does not mount on demand for plain existence checks, so an unmounted
/// share would always read as "unavailable" and scans would abort with "scan
/// root is unavailable". If the server asks for credentials, the standard GTK
/// mount dialog appears. `on_result` runs on the main thread.
pub fn mount_share_async(
    reference: &str,
    parent: Option<&gtk::Window>,
    on_result: impl FnOnce(Result<(), String>) + 'static,
) {
    let file = file(reference);
    net_debug(format!("mount requested: {reference}"));
    let mount_operation = gtk::MountOperation::new(parent);
    let reference_for_callback = reference.to_string();
    file.mount_enclosing_volume(
        gio::MountMountFlags::NONE,
        Some(&mount_operation),
        gio::Cancellable::NONE,
        move |result| {
            let outcome = match result {
                Ok(()) if query_exists(&reference_for_callback, true) => {
                    net_debug(format!("mount ok: {}", reference_for_callback));
                    Ok(())
                }
                Ok(()) => Err(format!(
                    "mounted, but the location is not reachable: {}",
                    reference_for_callback
                )),
                Err(error) => {
                    net_debug(format!(
                        "mount failed: {}: {}",
                        reference_for_callback, error
                    ));
                    Err(describe_mount_error(&reference_for_callback, &error))
                }
            };
            on_result(outcome);
        },
    );
}

fn describe_mount_error(reference: &str, error: &glib::Error) -> String {
    use gio::IOErrorEnum;
    match error.kind() {
        Some(IOErrorEnum::NotFound) | Some(IOErrorEnum::NotMounted) => format!(
            "server or share not found: {reference}. Check the server address and the share name."
        ),
        Some(IOErrorEnum::PermissionDenied) => {
            format!("access denied for {reference}. Check the credentials for the share.")
        }
        Some(IOErrorEnum::TimedOut) => {
            format!("the server did not respond in time: {reference}. Is the NAS online?")
        }
        _ => format!("{reference}: {error}"),
    }
}
```

---

## Step 2 — `src/db/photos.rs`: registration + queries

After `insert_discovered_folder`, add:

```rust
/// Register a network share (such as `smb://host/share`) as an independent
/// library root with a user-chosen display name. The share behaves like an
/// imported folder: it is scanned, indexed and availability-tracked through
/// the ordinary GIO source layer. Registered shares persist in the folders
/// table, so they remain visible (with cached thumbnails) while offline.
pub fn insert_network_share(connection: &Connection, uri: &str, name: &str) -> Result<i64> {
    let id = mark_import_root(connection, uri)?;
    connection.execute(
        "UPDATE folders SET name = ?1 WHERE id = ?2",
        params![name, id],
    )?;
    Ok(id)
}

/// Registered network share roots: URI-based folders the user added. Their
/// indexed subfolders stay descendants in the folders table but are presented
/// only inside the sidebar's Network Shares section.
pub fn network_shares(connection: &Connection) -> Result<Vec<Folder>> {
    Ok(folders(connection)?
        .into_iter()
        .filter(|folder| folder.imported_root && is_remote_path(&folder.path))
        .collect())
}

/// True when a folder path is a remote URI (smb://, nfs://, ...) rather than a
/// local filesystem path. Local-only sidebar sections and local-only behaviour
/// key off this.
pub fn is_remote_path(path: &str) -> bool {
    path.contains("://")
}
```

Removal is already safe: `db::remove_folder` only deletes DB rows (photos +
folder records, recursive) — it never touches the filesystem.

---

## Step 3 — `src/sidebar.rs`: section, rows, menu, tree exclusion

### 3a. Keys — after `FOLDER_WATCH_KEY`, add

```rust
const SHARE_LIST_KEY: &str = "picasa-sidebar-share-list";
const SHARE_OPEN_KEY: &str = "picasa-sidebar-share-open";
const SHARE_RETRY_KEY: &str = "picasa-sidebar-share-retry";
```

### 3b. `pub fn build(...)` signature — add three parameters after `on_folder_watch`

```rust
    on_add_share: Rc<dyn Fn()>,
    on_open_share: Rc<dyn Fn(Folder)>,
    on_retry_share: Rc<dyn Fn(Folder)>,
```

### 3c. In `build()`, inside the unsafe block that stores the folder callbacks
(`folder_list.set_data(FOLDER_REFRESH_KEY, ...)`), clone the two shared ones:

```rust
        folder_list.set_data(FOLDER_REFRESH_KEY, on_refresh_folder.clone());
        folder_list.set_data(FOLDER_STATISTICS_KEY, on_folder_statistics);
        folder_list.set_data(FOLDER_REMOVE_KEY, on_remove_folder.clone());
```

### 3d. In `build()`, after `folder_section.append(&folder_revealer);`
(`folder_section` is the Paned end child), add the section. It must live
INSIDE the Folders pane so collapsing Folders moves it up:

```rust
    // NETWORK SHARES: registered remote sources (Phase 1: SMB). Kept strictly
    // separate from local Folders.
    let (share_heading, _share_indicator) = collapsible_heading(
        "Network Shares",
        Some(on_add_share.clone()),
        "Add Network Share",
        true,
        None,
    );
    share_heading.add_css_class("sidebar-sticky-heading");
    let share_list = section_list();
    connect_filter_list(&share_list, on_filter.clone(), filter_syncing.clone());
    unsafe {
        outer.set_data(SHARE_LIST_KEY, share_list.clone());
        share_list.set_data(SHARE_OPEN_KEY, on_open_share);
        share_list.set_data(SHARE_RETRY_KEY, on_retry_share);
        share_list.set_data(FOLDER_REFRESH_KEY, on_refresh_folder);
        share_list.set_data(FOLDER_REMOVE_KEY, on_remove_folder);
    }
    folder_section.append(&share_heading);
    folder_section.append(&share_list);
```

(If `on_filter` was moved earlier in the function, pass `on_filter.clone()` at
the earlier `connect_filter_list(&folder_list, ...)` call instead.)

### 3e. `populate_folders` — keep remote folders out of the local tree

Right at the top of the function body, before the `unsafe` data block:

```rust
    // Registered network shares live in their own sidebar section; the local
    // Folders tree never mixes URI sources in. refresh_folder_rows filters
    // before calling, but the initial build() path passes the full list.
    let folders_owned;
    let folders = if folders
        .iter()
        .any(|folder| crate::db::is_remote_path(&folder.path))
    {
        folders_owned = folders
            .iter()
            .filter(|folder| !crate::db::is_remote_path(&folder.path))
            .cloned()
            .collect::<Vec<Folder>>();
        &folders_owned
    } else {
        folders
    };
```

### 3f. `refresh_folder_rows` — filter + refresh the share section

```rust
    let folder_scroll_value = folder_scroll_value(scrolled);
    clear_list(&folder_list);
    // Registered network shares are presented in their own section; the
    // Folders tree stays strictly local.
    let local_folders: Vec<Folder> = folders
        .iter()
        .filter(|folder| !crate::db::is_remote_path(&folder.path))
        .cloned()
        .collect();
    populate_folders(&folder_list, &local_folders, &state, on_unavailable);
    refresh_network_shares(scrolled, folders, on_unavailable);
    restore_folder_scroll(scrolled, folder_scroll_value);
```

### 3g. Append the three functions at the end of the file

Copy them verbatim from the working tree (search for these names):
`fn refresh_network_shares`, `fn append_share_row`, `fn add_share_context_menu`.
They implement: clear + filter (`imported_root && is_remote_path`) + rows
(remote icon, display name, photo count, ⚠ offline badge, tooltip = URI,
`picasa-filter = SidebarFilter::Folder(id)` so clicks filter like folder rows)
and the right-click menu **Open / Retry Connection / Rescan / Remove from
Library** (callbacks read back from the list data keys stored in 3d; Rescan
reuses `FOLDER_REFRESH_KEY`, Remove reuses `FOLDER_REMOVE_KEY` — which routes
through the existing DB-only confirmation dialog).

---

## Step 4 — `src/window/dialogs.rs`: the Connect dialog

Add `pub fn show_add_network_share_dialog(parent: gtk::Widget, on_connect:
Rc<dyn Fn(String, String, String)>)` — a `gtk::Dialog` with fields
**Name** (optional), **Server** (required), **Share** (optional) and a
**Connect** default button. On Connect it validates (server required), strips
`smb://` / slashes from the inputs, and calls `on_connect(name, server, share)`.
Copy verbatim from the working tree. Key details:

- transient parent: resolve via `parent.ancestor(gtk::Window::static_type())`
  (the incoming widget may be any widget).
- `dialog.set_default_response(gtk::ResponseType::Accept);` (no Option).
- Enter in the entries triggers `button.activate()`.
- Hint text: guest/saved credentials, then browse shares and pick the folder.

---

## Step 5 — `src/window/build.rs`: slot + connect → mount → browse → register

### 5a. Declare the slot next to `import_folder_slot` (top of `build`)

```rust
    let add_network_share_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
```

### 5b. Implement it right before the `import_folder_slot.replace(...)` block.
Copy verbatim from the working tree (search
`add_network_share_slot_for_import`). What it does when the dialog calls
`on_connect(name, server, share)`:

1. Build the browse root: `smb://{server}/` — or `smb://{server}/{share}/` when
   a share was typed (some NASes disallow share enumeration, so jumping
   straight into the share must work).
2. `crate::source::mount_share_async(&browse_root, Some(&mount_parent), ...)`
   — gvfs mount, password dialog if the server asks. On error: `show_error`
   with the friendly message from `describe_mount_error`.
3. On success: `gtk::FileDialog` with
   `set_initial_folder(Some(&gio::File::for_uri(&browse_root)))` and
   `select_folder(Some(&parent_window), ...)` — the native chooser lists the
   shares on the server (or the share contents when a share was given) and the
   user picks the actual photo folder.
4. On pick: `uri = crate::source::reference(&file)` (an `smb://` URI, never a
   `/run/user/...` path), display name = Name field or the folder basename,
   `db::insert_network_share(...)`, `sidebar_refresh()`, then enqueue the scan
   through the existing import path
   (`job.authorize_photo_scan(PhotoScanRequestReason::ImportFolder)` +
   `pending.push_back(uri)` + `start_next_scan()`).

---

## Step 6 — `src/window/layout.rs`: wire the three callbacks into `sidebar::build`

At the `sidebar::build(...)` call site, after the folder-watch closure and
before `folder_display_mode`, add:

1. **Add**: dispatches `add_network_share_slot` (the slot is declared earlier
   in the same `build` function — layout.rs is `include!`d into it).
2. **Open**: `gtk::UriLauncher::new(folder.path.as_str())` +
   `.launch(Some(&parent_window), gio::Cancellable::NONE, ...)` — shows the
   share in the system file browser; errors via `show_error`.
3. **Retry Connection**: `crate::source::mount_share_async(&path,
   Some(&parent_window), ...)`; on any outcome
   `crate::source::refresh_availability()` + the sidebar refresh closure, and
   `show_error` on failure. All three callbacks are copied verbatim from the
   working tree (search `NETWORK SHARES: section + button opens`).

`mount_share_async` runs its dialog/callback on the main thread — no UI
blocking; the actual network wait is inside gvfs's async mount.

---

## Step 7 — Tests + verification

DB test (`src/db/tests.rs`): registration persists an imported root with the
display name, `network_shares` lists the root but not descendants or local
folders, re-adding renames in place, `remove_folder` empties the list.

```bash
cargo test network_share
cargo test          # full suite
cargo build
```

Manual verification:

```bash
# gvfs layer sanity (server reachability, share existence, enumeration):
gio info smb://10.0.0.1/epg
gio list smb://10.0.0.1/epg/

# app with traces:
PIC_DEBUG_NET=1 GDK_BACKEND=x11 ./target/debug/pic-rs
```

1. `+` under NETWORK SHARES → Server `10.0.0.1`, Share `epg` → Connect.
   `[net-debug] mount requested/ok` → the folder browser opens inside the share.
2. Pick a folder → the share appears in the sidebar and the scan starts
   (`SCAN COMPLETE` with the imported count, no `scan root is unavailable`).
3. Leave Share empty → the browser starts at the server root and shows the
   share list (works only if the NAS allows share enumeration).
4. Right-click share → Open (file browser), Retry Connection, Rescan, Remove
   from Library (removes only PIC's index).
5. Kill the NAS connection / unplug network → badge shows ⚠, thumbnails remain,
   app starts normally; Retry Connection brings it back without re-import.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `scan root is unavailable` right after adding | mount step missing or failed — check `[net-debug] mount failed` and the error text |
| Server root browse shows nothing | NAS disallows share enumeration — type the Share name instead |
| Sidebar freezes for a long time | URI probe timeout missing (`uri_query_exists`) |
| Share vanishes after restart | not stored via `insert_network_share`/`mark_import_root` (must be `imported_root = 1` in `folders`) |
| Files at risk on Remove | never call fs APIs in the remove path — only `db::remove_folder` |
