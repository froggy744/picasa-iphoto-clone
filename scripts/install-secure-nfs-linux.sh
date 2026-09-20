#!/usr/bin/env bash
# OPTIONAL: administrator-managed secure-export path. Not for Windows/macOS.
# Builds original restricted helper; preserves existing root-owned allowlist.
set -euo pipefail
[[ "$(uname -s)" == Linux ]] || { echo 'Linux only' >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ $EUID == 0 ]] || { echo 'Run as root AFTER reviewing source and config' >&2; exit 1; }
command -v setcap >/dev/null || { echo 'Install libcap utilities first' >&2; exit 1; }
command -v pkg-config >/dev/null
pkg-config --exists libnfs || { echo 'Install libnfs headers first' >&2; exit 1; }
if [[ ! -e /etc/pic-nfs-helper.conf ]]; then
  echo 'Missing /etc/pic-nfs-helper.conf; create a root-owned host-IP/export allowlist first.' >&2
  exit 1
fi
cc -std=c11 -O2 -Wall -Wextra -fPIE -pie \
  $(pkg-config --cflags libnfs) \
  "$ROOT/helper/pic-nfs-helper-secure-linux.c" \
  -o "$ROOT/helper/pic-nfs-helper.secure" \
  $(pkg-config --libs libnfs)
install -d -m 755 -o root -g root /usr/local/libexec
install -o root -g root -m 755 "$ROOT/helper/pic-nfs-helper.secure" /usr/local/libexec/pic-nfs-helper
chown root:root /etc/pic-nfs-helper.conf
chmod 644 /etc/pic-nfs-helper.conf
setcap cap_net_bind_service=ep /usr/local/libexec/pic-nfs-helper
getcap /usr/local/libexec/pic-nfs-helper
