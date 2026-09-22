#!/usr/bin/env bash
# One read-only diagnostic command for PIC's direct SMB and NFS transports.
# It never invokes GVfs, mount(2), FUSE or a desktop file-manager mount.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SMB_SRC="$ROOT/scripts/pic-smb-probe.c"
NFS_SRC="$ROOT/pic-nfs-probe.c"
BIN_DIR="${PIC_DIAGNOSTICS_BIN_DIR:-$ROOT/target/diagnostics}"
SMB_BIN="${PIC_SMB_PROBE_BIN:-$BIN_DIR/pic-smb-probe}"
NFS_BIN="${PIC_NFS_PROBE_BIN:-$BIN_DIR/pic-nfs-probe}"

mkdir -p "$BIN_DIR"
if [[ ! -x "$SMB_BIN" || "$SMB_SRC" -nt "$SMB_BIN" ]]; then
    gcc -O2 -Wall -Wextra -o "$SMB_BIN" "$SMB_SRC" $(pkg-config --cflags --libs smbclient)
fi
if [[ ! -x "$NFS_BIN" || "$NFS_SRC" -nt "$NFS_BIN" ]]; then
    gcc -O2 -Wall -Wextra -o "$NFS_BIN" "$NFS_SRC" $(pkg-config --cflags --libs libnfs)
fi

export PICASA_TRACE="${PICASA_TRACE:-}"

usage() {
    cat >&2 <<EOF
Usage:
  $0                         Discover SMB/NFS servers and list shares/exports
  $0 --scan [a.b.c]          Scan the local or specified /24
  $0 --exports HOST          List NFS exports from HOST
  $0 --nfs VERSION HOST EXPORT [DIR]
                             List an NFS export/subfolder (v3 or v4)
  $0 --probe VERSION HOST EXPORT FILE
                             Mount, stat, open and read one NFS file
  $0 --smb [SMB-URI]         List SMB shares/subfolders
  $0 [SMB-URI]               List SMB shares/subfolders

Examples:
  $0
  $0 --exports 10.0.0.1
  $0 --nfs 3 10.0.0.1 /mnt/4TBP /
  $0 --nfs 4 10.0.0.1 /mnt/4TBP /
  $0 --nfs 3 10.0.0.1 /mnt/4TBP /Other
  $0 smb://10.0.0.1/Photos
EOF
}

list_nfs_exports() {
    "$NFS_BIN" --exports "$1"
}

scan_all() {
    local scan_output
    if [[ -n "${1:-}" ]]; then
        scan_output="$($SMB_BIN --scan "$1" 2>&1)"
    else
        scan_output="$($SMB_BIN --scan 2>&1)"
    fi || {
        printf '%s\n' "$scan_output"
        return 1
    }
    printf '%s\n' "$scan_output"
    while IFS= read -r host; do
        [[ -n "$host" ]] || continue
        printf '\n==== NFS exports %s ====\n' "$host"
        list_nfs_exports "$host" || true
    done < <(sed -n 's/^\[NFS server\] \([^ ]*\).*/\1/p' <<<"$scan_output" | sort -u)
}

case "${1:-}" in
    "") scan_all "" ;;
    --help|-h) usage ;;
    --scan) scan_all "${2:-}" ;;
    --exports) [[ $# -eq 2 ]] || { usage; exit 2; }; list_nfs_exports "$2" ;;
    --probe)
        [[ $# -eq 5 ]] || { usage; exit 2; }
        "$NFS_BIN" "$2" "$3" "$4" "$5"
        ;;
    --nfs)
        [[ $# -ge 5 && $# -le 6 ]] || { usage; exit 2; }
        [[ "$2" == 3 || "$2" == 4 ]] || { usage; exit 2; }
        version="$2"; host="$3"; export_path="$4"; directory="${5:-/}"
        "$NFS_BIN" --list "$version" "$host" "$export_path" "$directory"
        ;;
    --smb) shift; exec "$SMB_BIN" "$@" ;;
    smb://*|//*|\\\\*) exec "$SMB_BIN" "$@" ;;
    *) usage; exit 2 ;;
esac
