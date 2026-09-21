# PIC on Linux

Flatpak is PIC's primary Linux package. It builds against the selected GNOME
runtime, so the resulting application does not depend on the host distribution's
GLIBC, GTK4 or libadwaita versions. The AppImage target remains available for
compatible systems.

## Build prerequisites

KDE Neon and Ubuntu:

```sh
sudo apt update
sudo apt install flatpak flatpak-builder cargo git tar
flatpak remote-add --user --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo
```

Fedora:

```sh
sudo dnf install flatpak flatpak-builder cargo git tar
flatpak remote-add --user --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo
```

Build the current checkout:

```sh
./build-linux.sh local --flatpak-only
```

The first fully online build can fetch the source, SDKs and native SMB/NFS
dependencies and populate the build caches:

```sh
./build-linux.sh github --branch rc4 --flatpak-only
```

Local mode is deliberately offline and therefore requires dependencies from an
earlier online build to be cached. Release tests run inside the Flatpak SDK and
any failure stops packaging. `--skip-tests` is intended only for diagnostic
builds, not published releases.

## Install and launch

Install or replace the generated bundle (substitute its actual filename):

```sh
flatpak install --user --reinstall ./dist/PIC-1.0.0-REVISION-x86_64.flatpak
flatpak run io.github.you.PicasaRs
```

These commands are the same on KDE Neon, Ubuntu and Fedora. Desktop menus may
need a logout/login after Flatpak is installed for the first time.

PIC retains access to its existing native library at
`~/.local/share/picasa-rs/library.db` and thumbnail cache under
`~/.cache/picasa-rs`. Its Flatpak permissions also cover external drives,
host-mounted NFS/SMB filesystems and host GVfs mounts. The corresponding host
GVfs backend (for example `gvfs-smb`) must be installed for a GVfs protocol to
be available.

## Optional AppImage

```sh
./build-linux.sh local --appimage-only
```

The AppImage is not the primary cross-distribution package because a binary
built on a newer distribution can require a newer GLIBC than the target system.
