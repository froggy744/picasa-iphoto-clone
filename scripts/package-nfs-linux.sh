#!/usr/bin/env bash
# Release/build step. Run on each supported Linux architecture + libc baseline.
# The end user receives the result as part of PIC, not a separately installed tool.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$ROOT/target/release}"
command -v cc >/dev/null
command -v pkg-config >/dev/null
pkg-config --exists libnfs || { echo 'Build dependency: libnfs-devel/libnfs-dev' >&2; exit 1; }
mkdir -p "$DEST/libexec"
cc -std=c11 -O2 -Wall -Wextra -fPIE -pie \
  -Wl,-rpath,'$ORIGIN' \
  $(pkg-config --cflags libnfs) \
  "$ROOT/helper/pic-nfs-helper-portable.c" \
  -o "$DEST/libexec/pic-nfs-helper" \
  $(pkg-config --libs libnfs)
chmod 755 "$DEST/libexec/pic-nfs-helper"
# A dynamic build must ship its libnfs SONAME beside the helper; the loader
# resolves $ORIGIN in AppImage/Flatpak and in normal local release layouts.
libnfs=$(ldd "$DEST/libexec/pic-nfs-helper" | awk '/libnfs[.]so/ {print $3; exit}')
if [[ -n "$libnfs" && -f "$libnfs" ]]; then
  bundled_libnfs="$DEST/libexec/$(basename "$libnfs")"
  # ldd resolves $ORIGIN to the already-bundled library on subsequent runs.
  # Never copy a library onto itself (cp exits 1 under set -e).
  if [[ "$(readlink -f "$libnfs")" != "$(readlink -f "$bundled_libnfs")" ]]; then
    cp -L "$libnfs" "$bundled_libnfs"
  fi
  chmod 644 "$bundled_libnfs"
fi
if ldd "$DEST/libexec/pic-nfs-helper" | grep -q 'not found'; then
  echo 'ERROR: unresolved helper dependency, do not publish this package' >&2
  exit 1
fi
printf 'PIC NFS helper bundled into: %s/libexec\n' "$DEST"
