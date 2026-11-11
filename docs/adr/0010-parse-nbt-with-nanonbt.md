# ADR-0010: Parse NBT with nanonbt

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

wgc parses one NBT document per chunk but needs only a handful of
fields: `DataVersion`, `InhabitedTime`, whether `blending_data` exists, and structure references. Those fields live at
the root since 21w43a and under `Level` before it. A tag of the wrong
type must count as missing rather than as an error
([0002](0002-delete-only-never-inhabited-chunks.md)), and broken NBT
must never crash the run.

## Decision

- Use nanonbt (MIT OR Apache-2.0), pinned to a git commit in
  `[workspace.dependencies]`.
- nanonbt is used inside a single module of `wgc-core`; the rest of the
  code sees plain types without the dependency.
- Read only the needed fields in a single pass over the document,
  looking at both the root and `Level` at once and skipping everything
  else. nanonbt's derive does not support optional fields or ignoring
  wrong types, so the low-level reader API is used.
- Match the game where it is lenient: the last duplicate key wins, key
  names are compared as bytes without validating the encoding, and
  trailing bytes after the root compound are ignored.

## Consequences

- No full tree is built for fields wgc never uses.
- Replacing the NBT library touches one module.
- A git dependency cannot be published to crates.io. wgc is
  distributed as GitHub Releases binaries and through
  `cargo install --git <repo> --locked`; both crates are
  `publish = false`. Publishing `wgc-core` is deferred.
- Parsing is fuzz-tested so that malformed input becomes a content
  error (C-2), not a panic.
