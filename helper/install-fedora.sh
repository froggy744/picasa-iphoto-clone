#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
command -v cc >/dev/null || { echo 'Install gcc first: sudo dnf install gcc libnfs-devel libcap' >&2; exit 1; }
command -v setcap >/dev/null || { echo 'Install libcap first: sudo dnf install libcap' >&2; exit 1; }
cc -O2 -Wall -Wextra -Werror -D_FORTIFY_SOURCE=2 -fstack-protector-strong \
   -fPIE -pie pic-nfs-helper.c -o pic-nfs-helper \
   $(pkg-config --cflags --libs libnfs)
sudo install -d -m 755 -o root -g root /usr/local/libexec
sudo install -o root -g root -m 755 pic-nfs-helper /usr/local/libexec/pic-nfs-helper
if [ ! -e /etc/pic-nfs-helper.conf ]; then
    sudo install -o root -g root -m 644 pic-nfs-helper.conf /etc/pic-nfs-helper.conf
else
    echo 'Preserving existing /etc/pic-nfs-helper.conf'
fi
sudo chown root:root /etc/pic-nfs-helper.conf
sudo chmod 644 /etc/pic-nfs-helper.conf
sudo setcap cap_net_bind_service=ep /usr/local/libexec/pic-nfs-helper
getcap /usr/local/libexec/pic-nfs-helper
