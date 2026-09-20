#!/usr/bin/env bash
# Run on macOS with Xcode CLT and build-time libnfs; sign .app only AFTER this.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$ROOT/target/release}"
if [[ "$DEST" == *.app ]]; then DEST="$DEST/Contents/Helpers"; else DEST="$DEST/libexec"; fi
mkdir -p "$DEST"
clang -std=c11 -O2 -Wall -Wextra \
  $(pkg-config --cflags libnfs) \
  "$ROOT/helper/pic-nfs-helper-portable.c" \
  -o "$DEST/pic-nfs-helper" \
  $(pkg-config --libs libnfs)
chmod 755 "$DEST/pic-nfs-helper"
# Bundle the actual installed libnfs, then point the helper at its neighbor.
old=$(otool -L "$DEST/pic-nfs-helper" | awk 'NR>1 && /libnfs[^ ]*[.]dylib/ {print $1; exit}')
if [[ -n "$old" ]]; then
  if [[ "$old" == @rpath/* ]]; then
    libdir=$(pkg-config --variable=libdir libnfs)
    actual="$libdir/$(basename "$old")"
  else actual="$old"; fi
  [[ -f "$actual" ]] || { echo "Missing runtime library: $actual" >&2; exit 1; }
  cp -L "$actual" "$DEST/$(basename "$actual")"
  install_name_tool -change "$old" "@executable_path/$(basename "$actual")" "$DEST/pic-nfs-helper"
fi
# A release engineer must inspect/transitively bundle any additional dylibs
# and re-sign everything as part of PIC's normal app signing/notarization.
echo 'Inspect dependencies before publishing:'
otool -L "$DEST/pic-nfs-helper"
printf 'PIC NFS helper bundled into: %s\n' "$DEST"
