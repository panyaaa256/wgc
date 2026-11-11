# ADR-0011: Keep docs behavioral and code self-explanatory

- Status: Accepted
- Date: 2026-11-11

## Context

The original specification (Japanese, `docs/spec.md` and `docs/spec/`)
mixed behavior with implementation: module names, Rust types and
signatures, crates per module, and step-by-step procedures. Code
comments and tests pointed back to spec sections instead of stating
their rule.

This caused two problems. AI agents working from the specs were pulled
toward the implementation the text described, even when the code had
moved on or a better design existed. And the specs duplicated the code,
so every refactor had to update both, and the two drifted.

## Decision

- Docs define behavior, external facts, and contracts: what wgc does
  and guarantees, the Minecraft formats it relies on, and the output it
  promises. They do not describe types, APIs, module structure, or
  algorithms.
- Code, comments, and tests never reference docs. Each comment states
  its rule itself, and code must be readable without the docs.
- User-facing IDs (T-1 to T-5, L-1, S-1 to S-8, C-1, C-2, V-1 to V-6,
  W-1, W-2) are defined in code, in rustdoc; docs use them but are not
  their source.
- The Japanese `docs/spec/` tree is replaced by:
  - [../../ARCHITECTURE.md](../../ARCHITECTURE.md): crate boundaries
    and dependency direction.
  - [../gc-rules.md](../gc-rules.md): what is deleted, kept, and
    checked.
  - [../world-format.md](../world-format.md): the world layout and file
    formats wgc reads.
  - [../json.md](../json.md): the machine-readable output contract.
  - These ADRs, for why each major decision was made.
- All docs are written in English. `README_ja.md` stays as a
  translation of the README.

## Consequences

- A refactor that keeps behavior changes no docs.
- Docs change only when behavior, an external fact, or a contract
  changes; that makes such changes visible in review.
- Implementation rationale that is not a behavior (why a particular
  crate or lock call was chosen) lives in ADRs, not in the behavioral
  docs.
- Japanese-speaking contributors lose the Japanese specification.
- ADRs 0001 to 0010 were written retroactively from the original
  specification at the same time.
