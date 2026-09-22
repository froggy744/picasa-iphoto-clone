# Portable Direct NFS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restore direct unprivileged libnfs access and remove PIC's mandatory privileged-helper dependency from the application and Linux packages.

**Architecture:** PIC uses the existing native `private_nfs` libnfs transport for NFS discovery and file operations. The standalone probe calls the same libnfs APIs directly. Packaging bundles libnfs as a runtime dependency and contains no privileged helper requirement.

**Tech Stack:** Rust, libnfs, libsmbclient, C probes, AppImage/Flatpak packaging scripts.

**Spec:** `docs/superpowers/specs/2026-09-22-portable-direct-nfs-design.md`

## Global Constraints

- No GVfs, kernel mounts, separate helper installation, sudo, or Linux capabilities.
- SMB behavior must remain unchanged.
- NFSv3 and NFSv4 must be tested through the direct unprivileged path.
- Do not modify the NAS automatically.
- Do not commit or push.

## Review Focus

- A direct NFS operation must never consult `/usr/local/libexec`; test source references and runtime helper lookup removal.
- NFSv3 export paths must mount as exports while NFSv4 paths must retain their protocol paths; test both probe modes.
- A reserved-port rejection must produce an actionable `insecure` export message.
- AppImage and Flatpak contents must include libnfs without requiring a helper or system path.
- SMB discovery and access must remain unchanged.

### Task 1: Direct application transport

**Files:**
- Modify: `src/network_shares.rs`
- Modify: `src/private_nfs.rs`
- Remove from application graph or delete: `src/nfs_transport.rs`
- Modify: `src/main.rs`
- Test: existing Rust unit tests plus new NFS routing/error tests

- [ ] Add failing tests proving non-root NFS operations route through `private_nfs` and classify reserved-port failures.
- [ ] Run the focused tests and verify the expected failures.
- [ ] Restore direct `private_nfs` list/stat/read/range-read calls.
- [ ] Remove helper-backed module registration and helper path/process code.
- [ ] Run focused tests and the full Rust suite.

### Task 2: Direct probe and diagnostics

**Files:**
- Modify: `pic-nfs-probe.c`
- Modify: `scripts/test-shares.sh`
- Modify: `scripts/pic-smb-probe.c` only if required by verification
- Modify: `tests/test_protocol.py` or add a direct-probe test script

- [ ] Add failing coverage for direct NFSv3 and NFSv4 probe operations and actionable failure text.
- [ ] Run it red against the current implementation.
- [ ] Make the probe perform the same direct libnfs operations as `private_nfs`.
- [ ] Keep `--exports`, `--list`, and file probe modes under `test-shares.sh`.
- [ ] Verify SMB discovery and all four NAS exports.

### Task 3: Packaging and obsolete helper cleanup

**Files:**
- Modify: `README-PORTABLE-NFS.md` and Linux packaging documentation
- Remove: secure-helper installer scripts, helper source/build files, obsolete helper binaries, helper-only tests, and helper-only packaging scripts
- Add: packaging assertions for no `/usr/local` helper dependency

- [ ] Search all packaging and runtime references before deleting helper files.
- [ ] Update AppImage layout and loader checks to bundle libnfs for the application.
- [ ] Verify Flatpak manifest/build metadata grants network access and includes libnfs without a helper.
- [ ] Remove confirmed-obsolete helper files and stale documentation.
- [ ] Run package scripts and inspect dependencies.

### Task 4: Final verification

- [ ] Run `cargo test` and `cargo build --release`.
- [ ] Run direct probe syntax/build tests and `tests/test_protocol.py`.
- [ ] Run SMB/NFS discovery diagnostics.
- [ ] Build/check AppImage and Flatpak packaging paths available in the repository.
- [ ] Confirm no runtime or packaging reference to `/usr/local/libexec/pic-nfs-helper` remains.
- [ ] Run `git diff --check` and report the NAS configuration recommendation without changing the NAS.
