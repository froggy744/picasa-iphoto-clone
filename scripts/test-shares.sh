#!/usr/bin/env bash
# Test SMB share/subfolder access using PIC's exact transport method
# (libsmbclient, OS-user + empty-password fallback), outside the app.
# Mirrors native/private_smb.c. With no arguments it auto-scans the local
# subnet for SMB/NFS servers and lists every SMB server's shares.
set -u
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pic-smb-probe.c"
BIN="${PIC_SMB_PROBE_BIN:-/tmp/pic-smb-probe}"

if [ ! -x "$BIN" ] || [ "$SRC" -nt "$BIN" ]; then
    if ! gcc -O2 -Wall -o "$BIN" "$SRC" $(pkg-config --cflags --libs smbclient); then
        echo "Build failed. Install deps: sudo dnf install gcc libsmbclient-devel" >&2
        exit 2
    fi
fi

export PICASA_TRACE="${PICASA_TRACE:-}"   # set to any value for debug traces
exec "$BIN" "$@"