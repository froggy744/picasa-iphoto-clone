# Direct portable NFS

PIC uses libnfs directly from the application process for read-only NFS
discovery, directory listing, stat, reads, and range reads. It does not use
GVfs, kernel mounts, FUSE, a privileged helper, `/usr/local/libexec`, or Linux
capabilities.

The standalone diagnostic uses the same unprivileged libnfs transport:

```sh
./scripts/test-shares.sh --exports 10.0.0.1
./scripts/test-shares.sh --nfs 3 10.0.0.1 /mnt/4TBP /
./scripts/test-shares.sh --nfs 4 10.0.0.1 /mnt/4TBP /
./scripts/test-shares.sh --probe 3 10.0.0.1 /mnt/4TBP \
  '/Other/Tat Sing/20190917_184453.jpg'
```

## NAS compatibility

Some NFS servers require clients to use reserved source ports. An ordinary
AppImage, Flatpak, or desktop process cannot guarantee those ports without
extra privilege. Do not work around that restriction with a local helper.

For a read-only LAN export intended for ordinary-user clients, ask the NAS
administrator to preserve its existing options and append `insecure`, for
example:

```text
/mnt/dietpi_userdata  *(ro,insecure)
/mnt/4TBP             10.0.0.0/24(ro,insecure)
/mnt/4TBS             10.0.0.0/24(ro,insecure)
/mnt/4TBM             10.0.0.0/24(ro,insecure)
```

These are examples, not commands to apply automatically. `insecure` disables
the server's reserved-source-port trust check. Keep exports read-only, retain
appropriate host restrictions and authentication, and review the NAS's exact
existing export options before changing them.

If the NAS continues to require reserved ports, PIC reports that the export
policy is incompatible with unprivileged direct NFS access.

## Packaging

The Linux application links to libnfs directly. AppImage packaging bundles
the required shared library beside the application, and Flatpak builds libnfs
inside the Flatpak SDK. Neither package invokes or installs a separate helper.

Build checks:

```sh
cargo test --release --locked --offline
cargo build --release --locked --offline
./build-linux.sh local --appimage-only
./build-linux.sh local --flatpak-only
```
