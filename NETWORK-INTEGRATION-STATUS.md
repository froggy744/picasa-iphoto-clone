# Direct network shares integration — status

Base: exact supplied `picasa-main-clean.zip` (not the network-share experiment).

IMPORTANT: **This tree is a safety baseline, NOT a completed SMB/NFS integration.**
The existing clean Picasa source cache has been guarded so a remote RAW original
can no longer silently persist at `~/.cache/picasa-rs/source/`.

The direct `libnfs` and `libsmbclient` backends, network picker, scanner routing,
metadata and cache identity, thumbnail queue, offline persistence, and lightbox
on-demand path still require integration and Fedora runtime tests. Never ship
or describe this guard-only source as a working network importer.

Original user-provided baseline remains unchanged at
`/mnt/data/picasa-main-clean.zip`.
