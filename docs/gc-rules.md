# GC rules

This document is the authoritative description of what wgc deletes, what it
keeps and why, when it refuses to write output, and how it verifies the output.

Related documents:

- [world-format.md](world-format.md): file locations per DataVersion, region
  layout, chunk NBT fields, and the error IDs `S-n`, `C-n`, `W-n`.
- [json.md](json.md): the shape of every JSON document wgc prints or writes.
- [README.md](../README.md): commands, options, exit codes, and the procedure
  for swapping the output in.
- [adr/](adr/): design decisions behind these rules.

## Overview

- wgc never writes to the source world. The only exception is taking the OS
  lock on `session.lock` ([ADR 0001](adr/0001-never-modify-the-source-world.md)).
- The only chunks deleted are chunks whose `InhabitedTime` is 0 and which no
  protection rule keeps. Anything wgc cannot judge is kept
  ([ADR 0002](adr/0002-delete-only-never-inhabited-chunks.md)).
- Kept chunks are written byte for byte; chunk data is never re-encoded
  ([ADR 0003](adr/0003-copy-kept-chunks-byte-for-byte.md)).
- `scan`, the `gc` estimate, and `gc --out` build the same plan with the same
  rules. `gc --out` builds the plan again when it runs.
- The output is marked complete only after it is written, durable, and
  verified against the source world
  ([ADR 0004](adr/0004-mark-output-complete-only-after-verification.md)).

## Terms

| Term | Meaning |
|---|---|
| World folder | A folder that contains `level.dat`. Layout detection is described in [world-format.md](world-format.md#world-layouts) ([ADR 0009](adr/0009-detect-layouts-like-sekai.md)) |
| Dimension folder | A folder that directly contains `region/`, `entities/`, and `poi/` |
| Kind | One of `region`, `entities`, `poi` |
| Region triple | The three files (one per kind) of the same dimension and region coordinates, including zero-size files ([world-format.md](world-format.md#file-names)). A `.mcc` file belongs to the entry of the same kind at the chunk position its name gives |
| Chunk position | The absolute chunk coordinates derived from the region coordinates and the index within the file |
| Verbatim copy | Copying a file to the output without changing its content |
| GC rules | The rules that delete chunks with `InhabitedTime = 0` (see [Region chunks](#region-chunks)) |
| Clean rules | The rules that drop zero-size region files, temporary player data files, and orphan entities/POI entries. `gc` always applies them |
| Plan | The decision of what to delete and how to write each file, built from the world without writing anything |

## Planning

The plan is built by reading the world. Nothing is written while it is built.

- The same input produces the same plan, including its order.
- The GC rules and the clean rules are always both applied. With
  `--dimension`, GC deletion happens only in the given dimensions; the clean
  rules apply to every dimension.
- Broken files and broken chunks are not failures. They are handled inside the
  plan (verbatim copy, content error).

### Region triples

- If any of the three files of a triple has a structural error
  ([world-format.md](world-format.md#structural-errors), `S-n`), the whole
  triple is copied verbatim and nothing in it is deleted. Every structural
  error found is recorded
  ([ADR 0008](adr/0008-copy-broken-region-triples-verbatim.md)).
- Every chunk in the `region` file is decompressed and read. A chunk that
  fails to decompress or to parse as NBT is recorded as a content error
  ([world-format.md](world-format.md#content-errors), `C-n`).
- In a verbatim-copied triple, readable chunks are still read and their
  structure references are collected (used for R below).

### InhabitedTime classes

| Class | Condition |
|---|---|
| Value | A value of 0 or more |
| Missing | The tag is absent or has a different type |
| Negative | A negative value. Treated as undecidable |
| Unreadable | The chunk failed to decompress or parse (content error) |

Only the class "Value" with value 0 can make a chunk a deletion candidate.
Which tag is read for which DataVersion is described in
[world-format.md](world-format.md#chunk-nbt).

### Which chunks are deleted

Within each dimension, the deleted chunks are determined by these sets:

1. **Candidates C**: chunks with `InhabitedTime` 0 in a triple that is not
   copied verbatim, in a dimension targeted by GC.
2. **Forceload protection F**: each force-loaded chunk
   ([world-format.md](world-format.md#forced-chunks)) and every chunk within
   Chebyshev distance 2 of it.
3. **Structure reference protection R**: the union of the structure starts
   referenced by
   - every chunk that is not a candidate (`InhabitedTime` positive, missing,
     negative, or unreadable; in a verbatim-copied triple; outside the GC
     target), and
   - every chunk kept by F.

   References cross triple boundaries. References are followed only once: the
   references of a chunk kept only by R are not followed.
4. **Deleted chunks D = C − F − R**.

A dimension whose force-loaded chunks cannot be determined is recorded in the
plan. `gc --out` treats it as T-5; `scan` and the estimate compute D without F
for that dimension.

### Region chunks

Only chunks in D are deleted. Every kept chunk records exactly one reason. When
several apply, the one with the lowest priority number is recorded.

| Priority | Keep reason |
|---|---|
| 1 | In a verbatim-copied triple |
| 2 | Outside the `--dimension` target |
| 3 | Content error (`InhabitedTime` is Unreadable) |
| 4 | `InhabitedTime` cannot be judged (Missing or Negative) |
| 5 | `InhabitedTime` is positive |
| 6 | Forceload protection (F) |
| 7 | Structure reference protection (R) |

### Entities and POI entries

In a verbatim-copied triple, every entry is kept. Otherwise, the fate of an
entry follows the `region` chunk at the same position:

| `region` chunk at the same position | Entry |
|---|---|
| Kept | Kept |
| Deleted | Deleted (deleted by GC) |
| Absent (no file, zero-size file, or empty entry) | Deleted (orphan entry) |

If the `region` file cannot be read, the triple is already copied verbatim. An
unreadable `region` file is never treated as "absent".

### Files

The action for each file of a triple is the first matching row:

| Condition (checked top to bottom) | Action |
|---|---|
| Size 0 | Not written |
| In a verbatim-copied triple | Verbatim copy |
| No entry is deleted | Verbatim copy |
| No entry is kept | Not written |
| Otherwise | Repacked with only the kept entries |

A `.mcc` file is not written only if the entry that refers to it is deleted;
otherwise it is copied. A `.mcc` file with no region file does not belong to a
triple and is copied verbatim like any other file.

### Non-region files

Temporary player data files in the player data folder (names matching
`^[0-9a-f-]{36}-\d+\.dat$`, see
[world-format.md](world-format.md#player-data)) are not written. Every other
non-region file is copied verbatim (the full list is in
[Writing](#writing)).

### Report-only items

These are counted and reported but never affect a decision:

- **Old-format chunks**: `region` chunks whose chunk DataVersion is lower than
  the DataVersion in `level.dat`, counted whether deleted or not.
  <!-- TODO: state whether chunks without a DataVersion count as old-format. -->
- **Orphan `.dat_old` files**: `<uuid>.dat_old` without a matching
  `<uuid>.dat`.

### Estimate

For each dimension and kind, the plan reports the current size (including
`.mcc` files) and the size after output. The size after output is the size of
the copied `.mcc` files plus:

| File action | Size |
|---|---|
| Verbatim copy | Current size |
| Repacked | `8192 + Σ ceil((4 + L) / 4096) × 4096`, summed over kept entries, where `L` is the entry length |
| Not written | 0 |

The plan also reports the total size of the non-region files that are copied
verbatim. The estimated total output size (used for the
[free space](#free-space) check) includes it.

### Old-terrain boundaries

Kept chunks that have `blending_data`
([world-format.md](world-format.md#chunk-nbt)) and are edge-adjacent (up,
down, left, or right) to a chunk in D are collected and grouped into
8-connected clusters. For each cluster, the plan reports the chunk count and
the range (minimum and maximum chunk position).

### Ordering

- Dimensions: ascending ID.
- Triples: ascending coordinates.
- Chunks and entries: ascending index.
- Non-region files that are not written: ascending path.

The same input produces results (decisions, terminal output, JSON) with the
same content and order.

## Trust checks

The GC rules assume that `InhabitedTime = 0` means no non-spectator player has
ever been within 128 blocks of the chunk (vanilla adds to `InhabitedTime` every
tick for chunks within entity-processing range that have a player within 128
blocks). The trust checks test this assumption
([ADR 0007](adr/0007-gate-output-on-trust-checks.md)).

- An unreadable file is not an error; the check becomes `unknown`.
- Only the scope relevant to the dimensions targeted by GC (`--dimension`, or
  all dimensions) is examined.

| ID | Check | Applies to | Scope |
|---|---|---|---|
| T-1 | Paper configuration | Bukkit layouts | Worlds containing a targeted dimension |
| T-2 | Mob spawning | Bukkit layouts with DataVersion ≥ 4325 (1.21.5) | Worlds containing a targeted dimension |
| T-3 | Player positions | All | Players in a targeted dimension |
| T-4 | Version | All | The whole world |
| T-5 | Force-loaded chunk data | All | Targeted dimensions |

### T-1: Paper configuration

A `fixed-chunk-inhabited-time` of 0 or more freezes the value of every chunk.
The check reads `chunks.fixed-chunk-inhabited-time` from the per-world
`paper-world.yml` and the default `paper-world-defaults.yml`
([world-format.md](world-format.md#paper-config-files)).

| Per-world value | Result |
|---|---|
| 0 or more | `fail` |
| `-1` | `pass` |
| `default`, key absent, or file absent | Decided by the defaults file: 0 or more is `fail`, anything else is `pass` |

A negative value is treated as disabled, like `-1`. If the YAML cannot be read
or a value has an unexpected type (including an intermediate node that is not a
map), the result is `unknown`. For `paper.yml` (before Paper 1.19), the same
rules apply to `world-settings.<world folder name>.fixed-chunk-inhabited-time`,
falling back to `world-settings.default.fixed-chunk-inhabited-time`.

### T-2: Mob spawning

On Paper 1.21.5 and later, `InhabitedTime` does not grow while the game rule
`spawn_mobs` (formerly `doMobSpawning`) is disabled
([PaperMC/Paper#12827](https://github.com/PaperMC/Paper/issues/12827); unfixed
as of 2026-11-09 and still present in Paper 26.3).

The check fails if mob spawning is disabled in a world's game rules
([world-format.md](world-format.md#game-rules)). If the format cannot be read
or the version is unknown, the result is `unknown`. Spigot and other non-Paper
servers do not have this bug, but they cannot be told apart from the files, so
continuing requires `--force`.

### T-3: Player positions

Players are read from `<uuid>.dat` in the player data folder and from
`Data.Player` in `level.dat` (present only when DataVersion < 4772)
([world-format.md](world-format.md#player-data)). The fields used are `Pos`,
`Dimension` (an Int `0`, `-1`, `1` means `minecraft:overworld`,
`minecraft:the_nether`, `minecraft:the_end`; the tag type tells the form), and
`playerGameType` (`3` is spectator).

- If the player data folder, a player file named by UUID, or `level.dat`
  cannot be read, the result is `unknown`. A missing player data folder is
  fine.
- The host's player file and `Data.Player` count as one player when their
  position and dimension are equal.

A player is skipped when any of these holds: spectator; `Dimension` is not in
the dimension map or is not targeted; a field cannot be read; the chunk
`(floor(x / 16), floor(z / 16))` does not exist; that chunk's `InhabitedTime`
is not in the class "Value" (Missing, Negative, Unreadable). A remaining player
whose chunk has `InhabitedTime` 0 is a match.

| Situation | Result |
|---|---|
| One or more matches | `fail` (every matching player is recorded) |
| No match, and at least one player checked | `pass` |
| No player checked | `unknown` |

### T-4: Version

`unknown` if the DataVersion in `level.dat` is greater than the newest
DataVersion wgc knows (`DV_MAX_KNOWN` in
[world-format.md](world-format.md#version-reference)).

### T-5: Force-loaded chunk data

`unknown` if the force-loaded chunks of any targeted dimension cannot be
determined ([world-format.md](world-format.md#forced-chunks)).

### Outcomes

- Each check results in `pass`, `fail`, `unknown`, or `not_applicable` (the
  world is outside the check's "Applies to" column).
- Results are ordered T-1 to T-5.
- A `fail` records every failing target. An `unknown` records one reason.

| Failing target | Recorded |
|---|---|
| T-1: a Paper setting of 0 or more | File, value |
| T-2: a world with mob spawning disabled | World folder |
| T-3: a player in a chunk with `InhabitedTime` 0 | UUID, dimension, chunk position |

| `unknown` reason | Recorded |
|---|---|
| A file cannot be read | Path |
| An unexpected value | Path, key |
| An unknown version | DataVersion |
| No player could be checked | Nothing |
| Force-loaded chunks cannot be determined | Dimension ID |

### How results are used

- **`scan` and the estimate**: every result is shown; nothing stops.
- **`gc --out`**: any `fail` or `unknown` stops the run before anything is
  written, unless `--force` allows that ID.

`gc --out` runs the checks twice:

- **Early check**, before reading all region files. It stops together with L-1
  (see [Run guarantees](#run-guarantees)). Unless files change in between, its
  result equals the post-plan check, including with `--dimension` and with
  chunks stored in external `.mcc` files.
- **Post-plan check**, against the built plan.

When `--force` allows T-3, the early check reads nothing for T-3 (an allowed
T-3 never stops the run). Whether T-3 was overridden (and listed in `forced`)
is decided by the post-plan check.

### `--force`

`--force` overrides all of T-1 to T-5 and L-1; `--force=<ids>` overrides only
the listed IDs.

- For each overridden ID that actually stops, wgc continues, warns
  prominently with the ID and its content, and records the ID in `forced` of
  `complete.json`.
- The warning is printed only when the run actually hit that stop and
  continued.
- Overriding T-5 deletes chunks without forceload protection; wgc says so.
- Overriding L-1 continues without a lock.

`--force` never overrides: the world being in use, an invalid output folder,
insufficient free space, a verification failure, or a layout error.

## Locking

wgc takes the same `session.lock` lock as the game, without waiting
([ADR 0005](adr/0005-lock-session-lock-like-the-game.md)).

- While wgc holds the lock, the game cannot open the world. While the game
  holds it, wgc cannot take it.
- When a world spans several world folders (Bukkit), every folder's
  `session.lock` is locked. The same folder is locked only once.
- The lock is held until reading the world and writing the output are done.
- wgc never creates `session.lock` and never changes its content.
- If the world is older than 20w14a (DataVersion 2524), wgc warns that it
  cannot detect whether the game is running, even when the lock was taken.

### Lock failures

| Failure | Meaning |
|---|---|
| In use | Another process (the game) holds the lock |
| Missing (L-1) | `session.lock` does not exist |
| Not writable | `session.lock` cannot be opened for writing (permissions, etc.) |
| Held by this process | wgc itself already holds the lock (reopening could release it, so it is refused) |
| Other I/O | Any other read or write failure |

| Command | In use | Missing (L-1) | Not writable | Other I/O |
|---|---|---|---|---|
| `gc --out` | Stop | Stop (with `--force`, continue without a lock and warn) | Stop (advise running as the same user as the server) | Abort with an error |
| `scan`, `gc` estimate | Warn that the lock could not be taken and continue | Same | Same | Same |
| `verify` | Fail | Not a failure (the lock is not in use) | Fail | Fail |

`scan` and the estimate try every `session.lock` before reading and hold the
locks they take until reading is done.

## Scan

`scan` reports the plan built without `--dimension` together with the trust
check results; it makes no decisions of its own. The `gc` estimate and result
report the same items. Every reported item is listed in [json.md](json.md);
the terminal layout is not a contract.

### InhabitedTime buckets (ticks)

| Bucket | Range |
|---|---|
| `zero` | 0 |
| `missing` | Tag absent or of a different type |
| `negative` | Negative |
| `unreadable` | Content error |
| `lt_1s` | 1–19 |
| `lt_10s` | 20–199 |
| `lt_1m` | 200–1199 |
| `lt_10m` | 1200–11999 |
| `lt_1h` | 12000–71999 |
| `lt_10h` | 72000–719999 |
| `ge_10h` | 720000 or more |

## Running `gc --out`

`gc --out` writes a new world to the output folder according to the plan and
verifies it against the source world. The source world is opened read-only,
except for the lock.

### Run guarantees

- All of the following are checked before the output folder is created. If
  any of them stops the run, the output folder is not created (nothing is
  written):
  - the [output folder](#output-folder) preconditions;
  - the `session.lock` lock;
  - the early trust check (T-1 to T-5) and L-1;
  - building the plan;
  - the post-plan trust check;
  - [free space](#free-space).
- Every stop not allowed by `--force` (T-1 to T-5, L-1) is detected before all
  region files have been read, and the run stops at that point. L-1 is judged
  together with the trust check results, and all stopping IDs are reported
  together. When `--force` allows T-3, T-3 is checked only against the plan.
- The post-plan trust check also stops the run on any stop not allowed by
  `--force`. Unless files change in between, its result equals the early
  check (this guards the case where L-1 was overridden and no lock is held).
- The state of the source world (the same content as `source-state.json`) is
  recorded before the world is read to build the plan, while holding the lock
  if it was taken. V-6 compares against this record.
- The lock is held until writing, verification, and records are finished, and
  released afterwards.
- If writing, verification, or recording fails, `failure.json` is written and
  the run ends.
- `complete.json` is written only after all output is written, verification
  has passed, and `source-state.json` has been written.

### Cancellation

- On Ctrl-C, wgc stops after finishing the current unit of work (one region
  triple, or one file), releases the lock, and ends as interrupted. An
  interrupted run is never treated as a success.
- The output folder is left with neither `complete.json` nor `failure.json`
  (incomplete output).
- If the interrupt arrives after the work has returned, `scan`, the estimate,
  and `verify` exit as interrupted without printing results; a `gc --out` that
  already succeeded is treated as a success, since its output is complete.
- A second Ctrl-C exits immediately.

### Output folder

- It must be a path that does not exist or an empty folder.
- It must not be inside the source world or contain it (compared after
  normalizing the existing part of each path).
- The part of the path that does not exist yet must not contain `..`.
- Vanilla: the output folder becomes the world folder. Bukkit: each world
  folder is placed directly under the output folder with the same name; other
  files of the server root are not written.
- `.wgc/` is created directly under the output folder for records.

### Free space

The run stops if `estimated total output size × 1.01 + 64 MiB` exceeds the
free space of the output folder (or of its nearest existing ancestor).

### Writing

Every file under each world folder is handled as follows:

| File | Action |
|---|---|
| A region file in the plan | As in [Files](#files) |
| A `.mcc` file in the plan | As in [Files](#files) |
| A non-region file the plan does not write | Not written |
| `session.lock` directly in a world folder | Not written (the game creates it on startup) |
| `.wgc/` directly in a world folder | Not written (records of an earlier run) |
| Anything else | Verbatim copy |

- Empty directories are created too.
- Symbolic links are followed and their targets are written. Several links in
  the same directory are each written at their own path. A link to one of its
  own ancestors (a loop) is an error. Following a link produces the "followed
  symbolic link" warning. A link whose target does not exist is not written
  and produces the "skipped special file" warning.
- Anything other than regular files and directories is not written and
  produces the "skipped special file" warning.
- Hard links are never used: Minecraft overwrites region files in place, so a
  hard link would also change the source world
  ([ADR 0006](adr/0006-never-hard-link-output-files.md)).
- Permission bits and modification times match the source file. When run as
  root, owner and group also match (on file systems without owners, this is a
  warning, not a failure). When not run as root and some file is owned by a
  different user than the one running wgc, wgc warns once that ownership could
  not be preserved. On Windows, only the read-only attribute is matched.

### Durability

When `complete.json` appears, every written file (content and metadata) and
every created directory is durable on disk. `complete.json` only ever appears
with its full content (a partially written `complete.json` is never left
behind), and once it is in place it is itself durable.

### Swapping the output in

`gc --out` on completion and `wgc verify` on success print the procedure with
the actual paths. They also mention when symbolic links were replaced by their
targets or ownership could not be preserved. The procedure
is in [README.md](../README.md#swapping-in-the-result).

## Verification

| ID | Check |
|---|---|
| V-1 | Repacked files have no structural errors |
| V-2 | The set of entries in each repacked file equals the kept entries in the plan |
| V-3 | The payload and timestamp of each kept entry in repacked files equal the source |
| V-4 | The size of each verbatim-copied file (including `.mcc` files) equals the source |
| V-5 | The set of files in the output folder equals the set of files that should be written, derived from the plan independently (`.wgc/` excluded) |
| V-6 | The state of the source world equals the state recorded before the plan was built |

Failures are ordered by output path. On failure, every failure is written to
`failure.json`.

### Records (`<out>/.wgc/`)

The shapes are in [json.md](json.md).

| File | Meaning |
|---|---|
| `source-state.json` | Relative path, size, and modification time (nanoseconds, as reported by `stat`) of every file under each world folder, including `session.lock`. Compared by V-6 and `wgc verify` |
| `complete.json` | The output is complete. Written only under the conditions in [Run guarantees](#run-guarantees) and [Durability](#durability). Output without it, or with `failure.json`, is incomplete |
| `failure.json` | Where the run failed (writing, verification, or recording) and the list of verification failures. Failed output is kept, not deleted, so it can be attached to a bug report |

### `wgc verify`

`wgc verify` only reads. Without `--source`, it uses `source` from
`source-state.json`. It checks in this order and stops at the first failure:

1. `complete.json` is missing or `failure.json` exists → **incomplete**.
2. `--source` is given and differs from the recorded `source` (both
   normalized) → **source mismatch**.
3. A fresh `stat` of everything under each world folder differs from
   `source-state.json` (a file added or removed, or a size or modification
   time changed) → **changed**, with the changed files and whether each was
   added, removed, or modified.
4. A world folder's `session.lock` is locked (see the `verify` row in
   [Lock failures](#lock-failures)) → **in use**.

If every check passes, the result is success and `forced` from
`complete.json` is returned.

## Warnings

Warnings never stop processing.

| Warning | When |
|---|---|
| Lock could not be taken | `scan` or the estimate continued without the lock |
| Continued without a lock | L-1 was overridden with `--force` |
| Cannot detect whether the game is running | The world is older than 20w14a (DataVersion 2524) |
| Ownership could not be preserved | An output file's owner could not be matched to the source (once) |
| Followed symbolic link | The output contains the target of a link (with the path) |
| Skipped special file | Something other than a regular file or directory, or a link with no target, was not written (with the path) |
| Check overridden | A check was ignored with `--force` (with the check ID) |

## Progress

Long operations report completed and total counts per stage: reading (region
triples), writing (files in the source world), and verification (files
written). The completed count never decreases.

## Failures

Each command distinguishes the failures below. Every I/O failure names the
path involved. The exit code column refers to
[README.md](../README.md#exit-codes).

### Layout (all commands)

Every layout failure ([world-format.md](world-format.md#misdirected-paths),
including `W-n`) exits with code 4.

### Planning (`scan`, estimate, `gc --out`)

| Failure | Meaning | Exit code |
|---|---|---|
| Read failure | The OS refuses to open or read a file (permissions, etc.). There is nothing to decide from, so the run stops | 4 |
| Layout error | Listing files failed (for example, two names for the same coordinates) | 4 |
| Interrupted | Interruption was requested | 130 |

### `scan`

| Failure | Meaning | Exit code |
|---|---|---|
| Non-UTF-8 path | A path to be reported is not valid UTF-8 | 4 <!-- TODO: taken from the current CLI; the former spec did not map this failure --> |
| Plan cannot be built | Building the plan failed (see [Planning](#planning-scan-estimate-gc---out)) | As in Planning |

### `gc --out`

| Failure | Meaning | Exit code |
|---|---|---|
| Output folder not empty | The output folder exists and is not an empty folder | 3 |
| Invalid output path | The part of the output path that does not exist yet contains `..` | 3 |
| Output overlaps source | The output folder is inside the source world or contains it | 3 |
| Lock failure | In use, not writable, or held by this process: 3. Other I/O: 4. Missing (L-1) counts as a stop instead | 3 / 4 |
| Stopped | A trust check or L-1 stopped the run (with the IDs not allowed by `--force`) | 3 |
| Insufficient free space | With the required and available space | 3 |
| Layout error | A layout failure | 4 |
| Plan failure | A [planning](#planning-scan-estimate-gc---out) failure | 4 |
| Verification failed | Any of V-1 to V-6 did not pass (with every failure) | 5 |
| Other I/O | Any other read or write failure | 4 |
| Interrupted | Interruption was requested | 130 |

### `wgc verify`

| Failure | Meaning | Exit code |
|---|---|---|
| Incomplete | `complete.json` is missing or `failure.json` exists | 5 |
| Source mismatch | The given source world differs from the record | 5 |
| Changed | Files of the source world changed after they were recorded | 5 |
| In use | `session.lock` is locked | 5 |
| Other I/O | The records or the source world cannot be read | 4 |
