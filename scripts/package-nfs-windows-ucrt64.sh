#!/usr/bin/env bash
# Execute under MSYS2 UCRT64 (NOT MINGW64/MSYS) with PIC's normal toolchain.
set -euo pipefail
[[ "${MSYSTEM:-}" == UCRT64 ]] || { echo 'Run inside MSYS2 UCRT64' >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$ROOT/target/x86_64-pc-windows-gnu/release}"
mkdir -p "$DEST/libexec"
gcc -std=c11 -O2 -Wall -Wextra \
  $(pkg-config --cflags libnfs) \
  "$ROOT/helper/pic-nfs-helper-portable.c" \
  -o "$DEST/libexec/pic-nfs-helper.exe" \
  $(pkg-config --libs libnfs)
# Windows loads DLLs from the executable's own directory. Bundle libnfs;
# PIC's installer must bundle its other UCRT/GTK runtime dependencies too.
shopt -s nullglob
found=0
for dll in /ucrt64/bin/libnfs-*.dll; do
  cp "$dll" "$DEST/libexec/"
  found=1
done
[[ "$found" == 1 ]] || { echo 'No libnfs runtime DLL found' >&2; exit 1; }
# Copy helper-only UCRT dependencies that ldd resolves outside Windows system.
ldd "$DEST/libexec/pic-nfs-helper.exe" | awk '/=> \/ucrt64\/bin\// {print $3}' | while IFS= read -r dll; do
  [[ -f "$dll" ]] && cp -n "$dll" "$DEST/libexec/" || true
done
printf 'PIC NFS helper bundled into: %s/libexec\n' "$DEST"
