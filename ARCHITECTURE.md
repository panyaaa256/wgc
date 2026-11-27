# Architecture

This document describes the high-level architecture of wgc. Read it to find
where things live and which rules the code must not break. It names modules and
types instead of linking them; use symbol search to jump to them. The exact
rules live in [`docs/gc-rules.md`](docs/gc-rules.md), the storage formats in
[`docs/world-format.md`](docs/world-format.md), the JSON contract in
[`docs/json.md`](docs/json.md), and the reasons behind major decisions in
[`docs/adr/`](docs/adr/).

## Bird's-eye view

wgc removes chunks of a Minecraft Java Edition world that no player has ever
been near (`InhabitedTime = 0`). It never touches the original world: it writes
a smaller copy to a new folder, verifies that copy against the original, and
leaves the swap to the user.

Every command runs the same pipeline and stops at a different point:

```
path ─► layout ─► lock ─► trust (early) ─► plan ─► trust (on plan) ─► write ─► verify ─► records
        └───────────── scan / gc estimate ─────────────┘                └──── gc --out only ────┘
```

1. **layout** recognizes the world (vanilla or Bukkit, before or after 26.1)
   and lists its dimensions, region triples and data files.
2. **lock** takes the same OS lock on `session.lock` that the game takes.
3. **trust** checks that `InhabitedTime` can be relied on (T-1..T-5); for
   `gc --out` this runs before any region file is read, so a refusal is cheap.
4. **plan** reads every region triple once and decides, per chunk and per
   file, what is deleted, what is kept and why. `scan`, the estimate and
   `gc --out` all report from the same plan.
5. **output** writes the plan to the new folder, verifies it (V-1..V-6) and
   writes `complete.json` only when everything passed.

`wgc verify` later re-checks an output against the world before the user swaps
it in.

## Code map

The workspace has four crates: `crates/core` (`wgc-core`, a library),
`crates/wgc` (`wgc`, a library), `crates/fs` (`wgc-fs`, a library) and
`crates/cli` (`wgc-cli`, the `wgc` binary).

### `crates/core`

Pure data handling and every decision made from it, `no_std` + `alloc`
([ADR-0013](docs/adr/0013-keep-the-core-crate-no-std.md)). Files are read
through `WorldSource`; types that name files are generic over the source's
path type.

- `source`: `WorldSource`, read-only access to a world's files. It has no way
  to write.
- `types`: shared value types (coordinates, dimension IDs, file kinds, the
  check IDs `GuardId`), and `Known` and `UnknownReason`.
- `versions`: the DataVersion constants the code branches on.
- `region`: parsing Anvil region files, decompressing chunks, structural
  checks (S-1..S-8), and repacking a region from kept payloads.
- `inflate`: zlib and gzip decoding with an output size limit.
- `nbt`: reading the few NBT fields wgc needs from chunks, `level.dat`, player
  data and forced-chunk files. The only module that uses the NBT library.
- `layout`: the recognized layout (`WorldLayout`, `Dimension`, the region
  triples of a dimension) and the files it points at: game rules, forced
  chunks, player data and Paper configuration.
- `plan`: the `Plan` and how it is built: analyzing one region triple,
  deciding every chunk of a dimension, the boundary warnings, player files and
  the size estimate.
- `trust`: the trust checks T-1..T-5.
- `report`: turning a plan and trust results into the JSON reports.

The `test-util` feature exposes builders for synthetic region files and NBT to
the tests of the other crates.

### `crates/wgc`

The steps of each command, `no_std` + `alloc` like `wgc-core`. They are
written against traits, so they do not depend on where the world is stored or
how work runs in parallel.

- `host`: `Runner` (independent work, possibly in parallel; `Sequential` runs
  it in order), `World` (a recognized world: its layout, files and locks) and
  `GcHost` (the output location, the source state, and writing, verifying and
  recording the output).
- `observe`: `Observer`, `Progress` and `Warning`, the channel through which
  long operations talk to the caller.
- `plan`: `build_plan`, and the trust checks that run before planning.
- `scan`: `wgc scan` and the `gc` estimate.
- `gc`: `gc --out` (`run_gc`): the order of the steps, `--force`, the free
  space check, and recording a failure.

### `crates/fs`

Everything that touches the host file system. Its `plan`, `trust`, `report`
and `output` modules run the `wgc-core` and `wgc` ones on the host.

- `source`: `FsSource`, the host file system as a `WorldSource`.
- `world`: `FsWorld`, a world on disk as a `World` and `GcHost`, and `Rayon`,
  a `Runner` on the `rayon` thread pool.
- `types`: the `wgc-core` and `wgc` types for host paths.
- `layout`: recognizing the world layout, listing region triples and walking
  every file under the world folders. Every later step finds files through
  the `WorldLayout` it returns.
- `lock`: `session.lock` locking.
- `sys`: platform calls (file locks, free space, privileges). The only module
  with `unsafe` code.
- `plan`, `trust`, `report`: the `wgc-core` and `wgc` steps for a world on
  disk, and paths relative to the input.
- `output`: checking the output folder, copying, verification, the `.wgc/`
  records, and `wgc verify`.

### `crates/cli`

- `args`: command-line parsing.
- `main`: runs a command, maps failures to exit codes, handles Ctrl-C.
- `messages`: every user-facing sentence, in one place.
- `ui`: terminal output: colors, progress bars, warnings and the summary.

## Invariants

- **The original world is never written.** The only operation on it besides
  reading is locking `session.lock`. Output goes to a folder that must be new
  or empty and must not overlap the world. No hard links (the game overwrites
  region files in place).
- **Only chunks with `InhabitedTime = 0` are deleted.** Anything wgc cannot
  decide (missing, mistyped, negative or unreadable values; broken region
  files) is kept.
- **Kept chunks are copied byte for byte.** Chunk data is never re-encoded.
- **`complete.json` means verified.** It is written last, after every output
  file is written, verified against the original and durable on disk. An output
  without it, or with `failure.json`, is incomplete.
- **All refusals come before the output folder exists.** Every stop that
  `--force` does not allow is found before anything is written.
- **Determinism.** The same input produces the same plan, the same output and
  byte-identical JSON, regardless of parallelism.
- **Only `cli` prints.** The libraries have no CLI dependencies; warnings and
  progress go through `Observer`, and every sentence a user reads is built in
  `cli::messages`. The libraries return typed errors; `cli` turns them into
  messages and exit codes.
- **`core` and `wgc` do no file system access.** They are `no_std`, and read
  the world only through `WorldSource`, which cannot write.
- **Dependencies point one way:** `cli` → `fs` → `wgc` → `core`.

## Cross-cutting concerns

**Errors.** Broken input is data, not an error: structural and content errors
end up in the plan and the report. Errors are reserved for conditions that
leave wgc unable to decide (an unreadable file, an unrecognizable layout) and
carry the path involved.

**Cancellation.** Long operations take an `AtomicBool` and check it between
units of work (a region triple, or one output file). A cancelled run returns a
`Cancelled` error and never writes `complete.json`.

**Parallelism.** Planning runs per region triple through a `Runner`, writing
and verification per file, with `rayon` on the host. Results are sorted by position before they are combined, so
parallelism never changes the output.

**User-facing IDs.** T-1..T-5 and L-1 (checks), S-1..S-8 and C-1/C-2 (broken
input), V-1..V-6 (verification) and W-1/W-2 (a misdirected path) appear in
output and are defined in the rustdoc of the type that carries them.

**Testing.** Region files are synthesized in tests; acceptance behavior is
covered by integration tests in `crates/cli/tests`, and every JSON shape is
pinned by golden tests ([`docs/json.md`](docs/json.md)). See
[`CONTRIBUTING.md`](CONTRIBUTING.md).

**Documentation.** Code, comments and tests do not refer to the documents
under `docs/`; they explain themselves
([ADR-0011](docs/adr/0011-keep-docs-behavioral-and-code-self-explanatory.md)).
