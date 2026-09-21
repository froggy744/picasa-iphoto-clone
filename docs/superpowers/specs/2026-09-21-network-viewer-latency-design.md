# Network Viewer Latency Design

## Goal

Make rapid lightbox navigation over direct SMB and NFS responsive by ensuring
the selected photo is never queued behind obsolete or speculative reads, and
by reusing bytes from completed stale reads instead of fetching them again.

## Confirmed bottleneck

Navigation itself is immediate and stale-generation protection is correct.
The viewer permits three decode workers, but each direct SMB/NFS backend
serializes reads through one process-wide session. Multiple viewer workers can
therefore queue invisibly on the transport mutex. In `smb.log3.txt`, warm NFS
reads usually completed in roughly 65–157 ms, while requests queued behind
older reads reported 459–647 ms. The trace starts NFS timing before the C mutex,
so this difference includes transport-slot waiting.

After a network read returns, decode cancellation notices that the request has
no consumers and discards the bytes. The existing decoded-texture cache is
populated only by a non-stale GTK completion callback. A later revisit must
therefore read the same original again even though the earlier read completed.

## Architecture

### Priority-aware network read gate

Add one bounded read gate per direct network protocol. A gate allows one active
read, matching the serialization already enforced by the corresponding native
session. Waiting foreground work has priority over prefetch. A waiting request
whose consumer disappears exits without entering the native transport.

An already-active `smbc_read` or `nfs_read` remains non-preemptible. Those APIs
are synchronous, and rewriting their session/FFI model is outside this patch.
Consequently, a new foreground request may wait for at most one active read,
but never for queued stale foreground work or queued prefetch.

### Bounded reusable source cache

Add an in-memory LRU for completed direct SMB/NFS source bytes. Its key is the
original URI plus the imported source fingerprint (`mtime`, `size_bytes`). The
cache is limited by entry count and total bytes; oversized originals bypass it.
Successful reads are inserted before cancellation is checked, allowing a stale
request's completed read to serve a later foreground decode.

The cache stores no persistent files, does not change SQLite, and is used only
for network originals with a valid fingerprint. A changed fingerprint is a
miss. Local files and RAW decoding retain their existing paths. The existing
bounded decoded-texture cache remains unchanged and is checked first.

### Scheduling and promotion

The existing exact-variant request registry remains authoritative for joining
and foreground promotion. Prefetch retains its 250 ms timer and starts only
when no selected decode is outstanding. If navigation occurs after prefetch
starts, its queued read is cancelled or deprioritized; promotion of the same
request changes its effective lane to foreground without losing ownership.

### Trace measurements

Under `PICASA_TRACE=1`, add correlated events for:

- navigation request to GTK display completion;
- decoded-texture and source-byte cache hits/misses;
- foreground/prefetch read-slot wait;
- network read duration and byte count;
- decode-only duration;
- cache insertion and eviction.

URIs remain credential-sanitized. Trace output stays disabled by default.

## Safety and compatibility

- Generation checks remain the final authority for GTK display.
- No thumbnail is displayed during lightbox navigation.
- Cache limits remain bounded and the decoded-texture limit is unchanged.
- Offline/error behavior propagates through the existing result path.
- Originals remain read-only on SMB/NFS and are never persisted locally.
- No scanner, importer, schema, editor, export, discovery, or unrelated UI
  behavior changes.

## Tests and measurement

Add deterministic tests proving:

- waiting foreground work runs before waiting prefetch;
- cancelled queued work never calls the reader;
- promotion changes queued work to foreground priority;
- completed stale bytes are reusable;
- fingerprint changes cause a cache miss;
- count/byte limits evict least-recently-used entries;
- older generations still cannot display.

Run the complete Rust test suite, release compilation, protocol tests, and
diff validation. Compare existing trace timings with a new trace from the same
release profile. If live SMB/NFS UI access is unavailable to automation, report
that limitation explicitly and provide the exact new trace fields needed for a
same-host follow-up capture rather than claiming an unmeasured improvement.
