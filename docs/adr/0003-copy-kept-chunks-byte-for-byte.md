# ADR-0003: Copy kept chunks byte for byte

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

To delete chunks, wgc has to rewrite region files. Decoding and
re-encoding the chunks it keeps would require handling every NBT
layout and compression scheme the game has ever written, and any
mistake would corrupt chunks that were supposed to survive untouched.
wgc only needs a few fields to decide, not a full model of the chunk.

## Decision

- Kept chunks are never re-encoded or recompressed. wgc decompresses a
  chunk only to read the fields it needs.
- A region file that loses entries is repacked from the original
  payload bytes (length, compression type, and data): kept chunks are
  laid out in index order from sector 2 without gaps, each padded with
  zeros to the 4096-byte boundary.
- Each kept chunk keeps its original timestamp; empty slots get 0.
- External chunk files (`c.<x>.<z>.mcc`) are copied unchanged unless
  their chunk is deleted.
- A file that loses no entries is copied unchanged. The region format
  is described in [../world-format.md](../world-format.md).

## Consequences

- The game reads kept chunks exactly as before, whatever their format
  version. wgc does not reimplement DataFixerUpper and does not upgrade
  old chunks; it only counts them.
- Verification can compare kept payloads and timestamps with the
  original byte for byte
  ([0004](0004-mark-output-complete-only-after-verification.md)).
- wgc does not recompress or otherwise shrink kept chunks; the only
  saving is the deleted chunks and the sectors they freed.
- Standalone defragmentation (repacking without deleting) is deferred.
