# ADR-0012: Pin JSON output with golden tests

- Status: Accepted
- Date: 2026-11-11

## Context

Scripts and AI agents read wgc's `--json` output (`wgc.scan/1`,
`wgc.gc/1`, `wgc.verify/1`) and the records under `<out>/.wgc/`
(`wgc.source-state/1`, `wgc.complete/1`, `wgc.failure/1`). A renamed
key or a changed value type breaks them silently, and nothing tied the
documented shapes to the code.

## Decision

- [../json.md](../json.md) is the machine-readable contract. It
  documents every schema with examples.
- Every schema is pinned by byte-identical golden tests: each test
  builds the report from hand-made values and compares the serialized
  output with a string literal using `assert_eq!`, as in
  TMCJapan/sekai.
- No snapshot library and no new dependency. `insta` was considered;
  plain literals need no extra crate and keep the expected output
  visible in the test itself.
- Incompatible changes (removing or renaming a key, changing a value's
  type) bump the schema number, for example `wgc.scan/1` to
  `wgc.scan/2`. Adding a key is compatible and keeps the number;
  readers must ignore keys they do not know.

## Consequences

- Any change to the output fails a test, so it cannot happen by
  accident; the author updates the literal and, when needed, the
  schema number and json.md together.
- Hand-made values keep the tests independent of real worlds and of
  the planning code, so they pin the output shape only. Whether wgc
  computes the right values is covered by other tests.
- Large outputs make long literals; tests use small values instead of
  full worlds.
- Readers that reject unknown keys are outside the contract and may
  break on compatible changes.
