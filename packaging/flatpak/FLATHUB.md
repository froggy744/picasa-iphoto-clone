# Flathub submission notes for PIC (`io.github.froggy744.PIC`)

## What exists today

- **App-id:** `io.github.froggy744.PIC` (default of `PIC_APP_ID` in the build
  scripts; must match `src/main.rs:60` and the `.desktop`/metainfo ids).
- **Binary:** `pic-rs` (Cargo package name; Flatpak `command`).
- **Runtime:** `org.gnome.Platform//50` + `org.gnome.Sdk//50` with
  `org.freedesktop.Sdk.Extension.rust-stable//25.08` (defaults of
  `PIC_GNOME_RUNTIME` / `PIC_FDO_RUST_RUNTIME` in `build-linux.sh`).
- **Manifests:** the build scripts generate a full Flatpak manifest at build
  time. `io.github.froggy744.PIC.json` in this folder is the committed,
  standalone mirror of it (plus the committed desktop/metainfo installs).

## Files in this folder

| File | Purpose |
| --- | --- |
| `io.github.froggy744.PIC.json` | Standalone Flatpak manifest (mirrors the script-generated one) |
| `io.github.froggy744.PIC.desktop` | Desktop entry (validated with `desktop-file-validate`) |
| `io.github.froggy744.PIC.metainfo.xml` | AppStream metainfo (validated with `appstreamcli validate`) |
| `pic-rs-launcher` | Committed copy of the launcher the build scripts generate |

## Before submitting to Flathub

1. **Rename the app-id.** `io.github.froggy744.PIC` contains a placeholder
   (`you`). Flathub requires reverse-DNS ids matching the repo/fork, e.g.
   `io.github.froggy744.PIC`. Renaming means updating, all together:
   `build.rs`-independent defaults in `build-linux.sh`,
   `build-linux-icons-only.sh`, `PIC-build-linux-one-script.sh`,
   `src/main.rs:60`, the `.desktop` file name + `Icon=`/`StartupWMClass`
   lines, the metainfo `<id>`/`<launchable>`, and this manifest's file name
   and `app-id`.
2. **Host screenshots.** `samples/` contains real UI captures (1490x950).
   Commit the chosen ones to the repo, then uncomment the `<screenshots>`
   block in the metainfo and replace the `file://` placeholders with
   `https://raw.githubusercontent.com/...` URLs (`appstreamcli validate`
   rejects `file://`).
3. **Switch the app source from `dir` to `git`.** For flathub-builder the
   `picasa-rs` module `{"type": "dir", "path": "../.."}` must become
   `{"type": "git", "url": "https://github.com/froggy744/picasa-iphoto-clone.git", ...}`.
4. **Vendor Rust crates for the sandbox.** Flathub builds run without
   network. Generate a sources list with
   `flatpak-cargo-generator.py` (from `flatpak-builder-tools`) from
   `Cargo.lock` and add it as extra module sources, replacing the
   local `cargo vendor` step the build script performs on the host.
5. **Releases section.** Add a new `<release>` entry (version + date + notes)
   for every future public release; Flathub rejects metadata whose latest
   release predates the submission by too long.
6. **Remove the local bundle build leftovers for review:** the manifest keeps
   `--share=network` (needed for SMB/NFS discovery via Avahi) and
   `--filesystem=host` (a local photo manager watching arbitrary folders);
   both are acceptable on Flathub but must be justified in the submission
   comment.

## Local validation commands

```sh
desktop-file-validate packaging/flatpak/io.github.froggy744.PIC.desktop
xmllint --noout packaging/flatpak/io.github.froggy744.PIC.metainfo.xml
appstreamcli validate packaging/flatpak/io.github.froggy744.PIC.metainfo.xml
python3 -m json.tool packaging/flatpak/io.github.froggy744.PIC.json >/dev/null
```
