# ADR-0004: Mark output complete only after verification

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

The user replaces the world with the output by hand
([0001](0001-never-modify-the-source-world.md)). An output folder that
looks finished but is not (a crash halfway through, a bug in repacking,
the source changing while wgc read it) would be swapped in and lose
data. The user needs one unambiguous signal that an output is safe to
use, and a check right before swapping that nothing has changed since.

## Decision

- Before reading region files, `gc --out` records the source state
  (relative path, size, and modification time in nanoseconds of every
  file in each world folder) while holding the lock.
- After writing, wgc verifies the output against the plan and the
  original:
  - V-1: repacked files have no structural errors.
  - V-2: repacked files hold exactly the planned kept entries.
  - V-3: kept payloads and timestamps match the original.
  - V-4: verbatim copies (including `.mcc`) match the original size.
  - V-5: the set of output files matches the expected set, derived
    independently from the plan.
  - V-6: the source state still matches the recorded one.
- `<out>/.wgc/complete.json` is written only after every file is
  written, verification passes, and `source-state.json` is written.
  At that point all output files, their metadata, and the created
  directories are durable on disk; `complete.json` appears only with
  its full content and is itself made durable.
- Any failure while writing, verifying, or recording writes
  `failure.json` instead. Failed output is kept, not deleted, so it can
  be attached to a bug report. An interrupted run leaves neither file.
- `wgc verify <out>` gates the swap. It is read-only and fails when
  `complete.json` is missing or `failure.json` exists, when the source
  differs from the recorded one, when any source file was added,
  removed, or changed since the record, or when `session.lock` is
  locked. The game rewrites `session.lock` on every start, so any start
  since the run shows up as a change.

## Consequences

- An output without `complete.json` is incomplete by definition; tools
  and users never have to guess.
- Costs: verification rereads the output, and durability needs fsync
  calls, which make the run slower.
- The record format is a contract in [../json.md](../json.md)
  ([0012](0012-pin-json-output-with-golden-tests.md)).
- Verification compares against the original, not against what the
  game would load; it cannot catch a wrong deletion decision, only a
  wrong output.
