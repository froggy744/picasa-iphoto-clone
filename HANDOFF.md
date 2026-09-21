# Viewer Optimization Handoff

## Branch and status

Work is on `viewer-optimization`; do not merge it into `main` or `rc4` yet.
The implementation compiles and its automated tests pass. A same-host release
trace against the real SMB/NFS shares is still required to measure perceived
navigation latency and complete the manual regression checklist.

## Implemented

- Existing exact-variant in-flight deduplication and foreground promotion are
  preserved. A foreground lease can join either foreground or prefetch work;
  cancelling the old prefetch lease does not cancel the promoted request.
- Direct SMB and NFS viewer reads now enter one priority-aware gate per
  protocol before the synchronous native call. Waiting foreground work runs
  before waiting prefetch, and cancelled waiters never enter the transport.
- Completed SMB/NFS source bytes are retained in a RAM-only LRU keyed by exact
  URI plus imported `mtime` and `size_bytes`. It is bounded to 16 entries and
  64 MiB. Invalid fingerprints and oversized originals bypass the cache.
- Successful bytes are cached before decode cancellation is checked. Thus a
  completed stale read can be reused after reversing direction rather than
  being fetched again.
- The network gate rechecks the source cache after waiting, preventing a
  queued request from rereading bytes completed while it waited.
- `PICASA_TRACE=1` now separates source-cache result, network-slot wait,
  native-read duration, CPU decode duration, and navigation-to-display time.
- Preview 4 behavior remains: the lightbox never substitutes a thumbnail
  while the selected full-quality image is loading. Generation checks remain
  the final guard against stale display. The 250 ms prefetch-only timer and
  decoded-texture cache limit are unchanged.

## Confirmed cause and transport limitation

The viewer allowed several workers, while each direct SMB/NFS session
serialized native reads. Requests therefore queued invisibly inside transport
locking; in the old traces, normal reads were often about 65–157 ms while a
selected request delayed behind older work reached roughly 459–647 ms.

The native `smbc_read` and `nfs_read` calls are synchronous. They cannot be
safely interrupted without a much larger transport/session rewrite. This
patch instead prevents obsolete queued work from entering native I/O and
ensures foreground waits behind at most the one native call already active.

## Verification completed

- `cargo test --quiet`: 253 passed, 0 failed, 17 ignored.
- `cargo check --quiet`: passed (existing warnings only).
- `cargo check --release --quiet`: passed (existing warnings only).
- `python3 tests/test_protocol.py`: passed; mock C helper covered
  stat/list/read/framing/traversal/error paths.
- Focused source tests cover foreground priority, dynamic prefetch promotion,
  queued cancellation, stale-byte reuse, fingerprint changes, and LRU bounds.
- Existing lightbox tests cover joining foreground requests, safe promotion,
  and stale-generation rejection.
- `cargo fmt -- --check` is not clean because the wider pre-existing tree is
  not rustfmt-formatted. No repository-wide formatting change was made.

## Still to verify manually

- Release-profile local JPEG viewing.
- Direct SMB and NFS navigation on Linux, including rapid forward/backward
  movement, reversing during a read, and revisiting a stale completion.
- Share disconnect/reconnect and unavailable originals.
- Lightbox open/close, no thumbnail flash, no stale display, and bounded
  memory during extended navigation.

The direct private SMB/NFS modules are Linux-only (`cfg(target_os = "linux")`).
The office Windows PC cannot reproduce these direct transports natively; use
the Fedora PC (or an equivalent Linux GUI environment with libsmbclient and
libnfs) for the live trace.

## Resume in a fresh Codex session

```bash
git fetch origin
git switch viewer-optimization
git pull --ff-only
cargo test --quiet
cargo check --release --quiet
PICASA_TRACE=1 cargo run --release 2> viewer-after.log
```

Navigate the same photo sequence used for `smb.log5.txt`, then compare:

- `PIC_VIEWER display_done ... navigation_ms=`: actual selected-photo latency.
- `PIC_VIEWER network_slot ... wait_ms=`: time waiting for native I/O access.
- `PIC_VIEWER network_done ... read_ms=`: native read duration after admission.
- `PIC_VIEWER source_cache ... action=hit|hit_after_wait|insert|evict`:
  successful reuse and bounded eviction.
- `PIC_VIEWER decode_stage source_ms=... cpu_ms=...`: I/O versus decode cost.
- `PIC_VIEWER request lane=foreground action=join|promote`: in-flight reuse.

Expected evidence for the old sequential-reread case is one network
`read_start`, followed later by a source-cache hit and no second network
`read_start` for the same URI/fingerprint. A selected request may wait for one
already-active synchronous read, but its slot wait must not include queued
prefetch reads.
