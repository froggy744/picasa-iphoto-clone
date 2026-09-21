# Network Viewer Latency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prioritize selected-photo SMB/NFS reads and reuse completed stale network bytes so rapid lightbox navigation is responsive.

**Architecture:** Keep the existing exact-variant request registry and decoded-texture cache. Add a foreground-aware single-reader gate and bounded fingerprinted byte LRU at the source boundary, then pass viewer lane/fingerprint data through the existing decoder and emit stage timings.

**Tech Stack:** Rust, GTK4/libadwaita, existing direct libsmbclient/libnfs FFI, Cargo tests.

**Spec:** `docs/superpowers/specs/2026-09-21-network-viewer-latency-design.md`

## Global Constraints

- Work only on `viewer-optimization`; never merge or modify `rc4` or `main`.
- Preserve Preview 4's no-thumbnail-flash behavior and immediate navigation.
- Keep the existing 250 ms prefetch timer and decoded-texture cache limits.
- Keep originals read-only and never persist network originals locally.
- Do not rewrite SMB/NFS transports or change unrelated import/UI/database behavior.
- Trace only under `PICASA_TRACE=1` and never expose credentials.

## Review Focus

- A promoted prefetch must acquire foreground priority without losing its work.
- A cancelled queued request must not invoke the network reader.
- An active synchronous read may finish, but no queued stale work may run before foreground.
- Unknown or changed fingerprints must not return stale source bytes.
- Cache count and byte budgets must remain bounded for oversized originals.

---

### Task 1: Priority-aware source read gate

**Files:**
- Modify: `src/source.rs`
- Test: `src/source.rs`

**Interfaces:**
- Produces: `ViewerReadLane`, `ViewerReadContext`, and `read_for_viewer(reference, context)`.
- Consumes: existing `read(reference)` and direct-network URI detection.

- [x] Write tests proving foreground overtakes waiting prefetch and cancelled waiters never call the reader.
- [x] Run the focused tests and verify they fail because the gate API is absent.
- [x] Implement one gate per direct protocol with one active reader, dynamic lane checks, cooperative queued cancellation, and slot-wait tracing.
- [x] Run focused tests and verify they pass.

### Task 2: Fingerprinted bounded source-byte cache

**Files:**
- Modify: `src/source.rs`
- Modify: `src/lightbox.rs`
- Modify: `src/lightbox/render.rs`
- Modify: `src/thumbnail/viewer.rs`
- Test: `src/source.rs`, `src/lightbox/tests.rs`

**Interfaces:**
- Consumes: `ViewerReadContext` from Task 1 and `PhotoObject::mtime/size_bytes`.
- Produces: source cache lookup/insert with exact URI+mtime+size validation and bounded LRU eviction.

- [x] Write tests proving completed stale bytes are reusable, changed fingerprints miss, and count/byte budgets evict LRU entries.
- [x] Run focused tests and verify the missing cache behavior fails.
- [x] Add the bounded network-only byte cache; insert immediately after successful reads before decode cancellation checks.
- [x] Pass fingerprint/lane through the existing viewer request without changing exact-variant dedup or generation ownership.
- [x] Run focused tests and verify they pass.

### Task 3: End-to-end latency tracing and verification

**Files:**
- Modify: `src/lightbox/render.rs`
- Modify: `src/thumbnail/viewer.rs`
- Create if live validation remains unavailable: `HANDOFF.md`

**Interfaces:**
- Consumes: source read/cache timing from Tasks 1–2.
- Produces: gated navigation-to-display, source-cache, slot-wait, network, decode-only, and eviction events.

- [x] Add trace fields for navigation-to-display, cache result, slot wait, network duration, and decode-only duration.
- [x] Run focused tests, `cargo test --quiet`, `cargo check --release`, `python3 tests/test_protocol.py`, and diff validation.
- [x] Compare `smb.log3.txt`/`smb.log5.txt` baseline figures with any available same-profile live trace; explicitly mark live after-data unavailable if it cannot be collected automatically.
- [x] Review the complete branch diff for stale-display, promotion, bounded-memory, and offline-error regressions.
- [x] Commit only implementation/spec/plan/handoff files and push `viewer-optimization`.
