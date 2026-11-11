# ADR-0008: Copy broken region triples verbatim

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

Real worlds contain damaged region files: truncated by a crash,
overlapping sectors, missing external `.mcc` files. The three files of
a region triple (`region`, `entities`, `poi` at the same coordinates)
are linked: `entities` and `poi` entries are kept or deleted together
with the region chunk at the same position. Repacking a file whose
structure is wrong could drop data, and repacking only part of a
triple could leave entities or POIs without their chunk.

## Decision

- A structural error in any file of a triple makes the whole triple a
  verbatim copy. Nothing in it is deleted. The structural errors are
  S-1 to S-8 (file shorter than the header, malformed or overlapping
  sector ranges, a length that runs past the file, a missing `.mcc`,
  and so on; see [../world-format.md](../world-format.md)).
- A chunk that cannot be decompressed or parsed (C-1, C-2) is kept and
  its payload is copied unchanged; the rest of the file is processed
  normally.
- Broken input never aborts the run. Every structural and content
  error is reported by `scan`, the estimate, and `gc`.
- A zero-length region file is not a structural error; it is treated
  as a file without chunks.

## Consequences

- wgc never makes a damaged world worse; it also does not repair it.
- A single bad sector in one file prevents any deletion in its triple,
  so damaged worlds shrink less.
- Readable chunks in a verbatim triple are still read, so their
  structure references keep protecting chunks elsewhere.
- The run still stops when the OS refuses to open or read a file,
  because there is nothing to decide with.
