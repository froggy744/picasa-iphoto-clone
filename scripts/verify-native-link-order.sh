#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT

cd "$ROOT"

set +e
# A changing tracked pkg-config value makes Cargo rerun build.rs and relink PIC
# without deleting debug or release artifacts. Any set value selects dynamic NFS.
LIBNFS_DYNAMIC="pic-link-order-check-${RANDOM}-${RANDOM}" cargo build -vv >"$LOG" 2>&1
build_status=$?
set -e

output_file="$(find target/debug/build -path '*/pic-rs-*/output' -type f -printf '%T@ %p\n' \
    | sort -nr \
    | head -n 1 \
    | cut -d' ' -f2-)"

if [[ -z "$output_file" ]]; then
    echo "ERROR: Cargo did not produce build-script output for pic-rs." >&2
    tail -n 80 "$LOG" >&2
    exit 1
fi

archive_line="$(grep -n 'cargo:rustc-link-lib=static=pic_private_transports' "$output_file" | cut -d: -f1 || true)"
smb_line="$(grep -n 'cargo:rustc-link-lib=smbclient' "$output_file" | cut -d: -f1 || true)"
nfs_line="$(grep -n 'cargo:rustc-link-lib=nfs' "$output_file" | cut -d: -f1 || true)"

if [[ -z "$archive_line" || -z "$smb_line" || -z "$nfs_line" ]]; then
    echo "ERROR: Missing expected native link directive in $output_file." >&2
    grep 'cargo:rustc-link-\(lib\|search\)' "$output_file" >&2 || true
    exit 1
fi

if ((archive_line >= smb_line || archive_line >= nfs_line)); then
    echo "ERROR: Native transport archive must be linked before SMB/NFS dependencies." >&2
    grep -n 'cargo:rustc-link-lib=\(static=pic_private_transports\|smbclient\|nfs\)' "$output_file" >&2
    exit 1
fi

rustc_command="$(grep 'Running.*rustc.*--crate-name pic_rs' "$LOG" | tail -n 1 || true)"
archive_position="$(grep -bo -- '-l static=pic_private_transports' <<<"$rustc_command" | cut -d: -f1 || true)"
smb_position="$(grep -bo -- '-l smbclient' <<<"$rustc_command" | cut -d: -f1 || true)"
nfs_position="$(grep -bo -- '-l nfs' <<<"$rustc_command" | cut -d: -f1 || true)"

if [[ -z "$archive_position" || -z "$smb_position" || -z "$nfs_position" ]]; then
    echo "ERROR: Missing expected native library in rustc's final link arguments." >&2
    exit 1
fi

if ((archive_position >= smb_position || archive_position >= nfs_position)); then
    echo "ERROR: rustc placed SMB/NFS before the native transport archive." >&2
    exit 1
fi

if ((build_status != 0)); then
    echo "ERROR: cargo build failed after emitting the expected native link order." >&2
    tail -n 80 "$LOG" >&2
    exit "$build_status"
fi

echo "rustc native link order:"
echo "  -l static=pic_private_transports -l smbclient -l nfs"
