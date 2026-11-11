# JSON output

The machine-readable contract for scripts and AI agents.

- `scan`, `gc` and `verify` accept `--json`. With `--json`, stdout carries exactly one JSON document and the human-readable output goes to stderr. JSON output is never colored.
- `gc --out` writes record files under `<out>/.wgc/`.
- Exit codes do not depend on `--json` (see [README](../README.md#exit-codes)).

## Documents

| `schema` | Produced by |
|---|---|
| `wgc.scan/1` | `wgc scan --json` |
| `wgc.gc/1` | `wgc gc --json` (both estimate and output) |
| `wgc.verify/1` | `wgc verify --json` |
| `wgc.source-state/1` | `<out>/.wgc/source-state.json` |
| `wgc.complete/1` | `<out>/.wgc/complete.json` |
| `wgc.failure/1` | `<out>/.wgc/failure.json` |

## Conventions

- Keys and enum values are `snake_case`.
- Paths are relative to the input path, with `/` as the separator; the input path itself is `"."`. Paths outside the input path (for example server configuration when a world folder was given) are printed as they are.
- `source` and `out` are the exceptions: canonical absolute paths.
- If any path is not valid UTF-8, no JSON is produced and the command fails.
- Lists are never truncated. The same input produces byte-identical JSON. Lists are ordered by dimension ID, then region coordinates, then index within the file, then path.
- Check IDs are strings of the form `T-1`..`T-5` and `L-1`; structural error IDs `S-1`..`S-8`; verification IDs `V-1`..`V-6` (see [gc-rules.md](gc-rules.md) and [world-format.md](world-format.md)).

## `wgc.scan/1`

```json
{
  "schema": "wgc.scan/1",
  "tool_version": "0.1.0",
  "layout": { "kind": "bukkit_legacy", "base": "world", "world_folders": ["world", "world_nether"] },
  "world_data_version": 4189,
  "lock": "acquired",
  "trust": [
    { "id": "T-1", "status": "pass" },
    { "id": "T-2", "status": "not_applicable" },
    { "id": "T-3", "status": "fail", "details": [
      { "kind": "player_in_zero_chunk", "uuid": "069a79f4-44e9-4726-a5be-fca90e38aaf5", "dimension": "minecraft:overworld", "pos": [-12, 40] }
    ] },
    { "id": "T-4", "status": "unknown", "reason": { "kind": "unknown_version", "data_version": 9999 } },
    { "id": "T-5", "status": "unknown", "reason": { "kind": "forced_chunks_unknown", "dimensions": ["minecraft:overworld"] } }
  ],
  "other_files": {
    "omitted": [{ "path": "world/playerdata/069a79f4-44e9-4726-a5be-fca90e38aaf5-123.dat", "reason": "player_temp_file" }],
    "dat_old_without_dat": ["world/playerdata/8667ba71-b85a-4004-af54-457a9734eed7.dat_old"]
  },
  "notices": ["pregenerated_chunks", "boundary_general", "run_official_optimize"],
  "dimensions": [
    {
      "id": "minecraft:overworld",
      "dir": "world",
      "size": { "region": 1048576, "entities": 65536, "poi": 8192 },
      "chunks": { "total": 175, "older_data_version": 12 },
      "inhabited_time": { "zero": 120, "missing": 1, "negative": 2, "unreadable": 3, "lt_1s": 4, "lt_10s": 5, "lt_1m": 6, "lt_10m": 7, "lt_1h": 8, "lt_10h": 9, "ge_10h": 10 },
      "gc": {
        "delete": 100,
        "protected": { "forceload": 4, "structure_ref": 16 },
        "size_after": { "region": 524288, "entities": 32768, "poi": 4096 },
        "emptied_files": ["world/region/r.1.1.mca"]
      },
      "integrity": {
        "structural": [
          { "path": "world/region/r.0.0.mca", "id": "S-1", "index": null },
          { "path": "world/region/r.0.0.mca", "id": "S-7", "index": 5 }
        ],
        "content": [{ "pos": [3, -4], "kind": "decompress", "detail": "invalid zlib stream" }],
        "verbatim_triples": [[0, 0]]
      },
      "orphans": { "entities": 7, "poi": 9 },
      "empty_files": ["world/region/r.2.2.mca"],
      "boundary_warnings": [{ "chunks": 24, "min": [-3, 0], "max": [5, 7] }]
    }
  ]
}
```

| Key | Value |
|---|---|
| `layout.kind` | `vanilla_legacy`, `vanilla`, `bukkit_legacy`, `bukkit` (see [world-format.md](world-format.md)) |
| `layout.base` | The Bukkit `<base>`; `null` for vanilla |
| `world_data_version` | DataVersion of `level.dat`; `null` when absent |
| `lock` | `acquired`, `in_use` (in use, or already held by this process), `missing` (no `session.lock`), `not_writable`, `error` (other I/O failure) |
| `trust[].status` | `pass`, `fail`, `unknown`, `not_applicable` |
| `trust[].details` | Only with `fail`: an array of failed targets (below) |
| `trust[].reason` | Only with `unknown`: why the check could not be confirmed (below) |
| `other_files.omitted[].reason` | `empty_file` (size 0), `no_entries_left` (nothing left to keep), `player_temp_file` (a leftover temporary player-data file) |
| `other_files.dat_old_without_dat` | `<uuid>.dat_old` files whose `<uuid>.dat` is missing |
| `notices` | IDs of fixed notices: `pregenerated_chunks`, `boundary_general`, `run_official_optimize`, `no_trash` |
| `dimensions[].chunks.older_data_version` | Chunks stored in a format older than the world |
| `dimensions[].inhabited_time` | Chunk counts per InhabitedTime bucket (see [gc-rules.md](gc-rules.md)) |
| `dimensions[].gc.emptied_files` | Files not written because no entry is left to keep |
| `dimensions[].integrity.structural[]` | Structural errors; `index` is `null` for `S-1` |
| `dimensions[].integrity.content[]` | Content errors: `{ "pos": [x, z], "kind": "decompress" \| "nbt", "detail": "…" }`. `detail` is a human-readable message with no guaranteed format |
| `dimensions[].integrity.verbatim_triples[]` | Region coordinates `[x, z]` of triples copied verbatim |
| `dimensions[].orphans` | entities/poi entries whose region chunk is missing |
| `dimensions[].boundary_warnings[]` | Clusters where deleted chunks meet older terrain: chunk count and bounding box |

### Failed targets (`details[]`)

| `kind` | Fields | Check |
|---|---|---|
| `paper_fixed_inhabited_time` | `file`, `value` | T-1 |
| `mob_spawning_disabled` | `world_folder` | T-2 |
| `player_in_zero_chunk` | `uuid`, `dimension`, `pos` | T-3 |

### Unknown reasons (`reason`)

| `kind` | Fields |
|---|---|
| `unreadable_file` | `path` |
| `unexpected_value` | `path`, `key` |
| `unknown_version` | `data_version` |
| `no_players_checked` | none |
| `forced_chunks_unknown` | `dimensions` |

## `wgc.gc/1`

Every `wgc.scan/1` key, followed by:

```json
{ "schema": "wgc.gc/1", "…": "all wgc.scan/1 keys", "mode": "output", "out": "/abs/out", "forced": ["T-1", "T-5", "L-1"], "dimensions_targeted": ["minecraft:overworld"] }
```

| Key | Value |
|---|---|
| `mode` | `estimate` (no `--out`) or `output` |
| `out` | Absolute path of the output; `null` for `estimate` |
| `forced` | Check IDs ignored with `--force` |
| `dimensions_targeted` | Values of `--dimension`; `null` when not given |

## `wgc.verify/1`

```json
{ "schema": "wgc.verify/1", "result": "ready", "source": "/abs/server", "changed": [], "forced": ["T-1", "L-1"] }
{ "schema": "wgc.verify/1", "result": "changed", "source": null, "changed": ["world/level.dat", "world/region/r.0.0.mca"], "forced": [] }
```

| Key | Value |
|---|---|
| `result` | `ready`, `incomplete`, `source_mismatch`, `changed`, `in_use`, `error` (reading failed) |
| `source` | Absolute path of the source world; `null` unless `ready` |
| `changed` | With `changed`: paths of the files that changed |
| `forced` | `forced` from `complete.json` |

## Records under `<out>/.wgc/`

### `wgc.source-state/1`

Relative path, size and modification time (nanoseconds) of every file under each world folder, `session.lock` included.

```json
{
  "schema": "wgc.source-state/1",
  "source": "/abs/server",
  "layout": "bukkit_legacy",
  "world_folders": ["world", "world_nether"],
  "files": [
    { "path": "world/level.dat", "size": 1532, "mtime_ns": 1794312896123456789 },
    { "path": "world/region/r.0.0.mca", "size": 8192, "mtime_ns": -1 }
  ]
}
```

### `wgc.complete/1`

Marks the output as complete (see [gc-rules.md](gc-rules.md)).

```json
{
  "schema": "wgc.complete/1",
  "tool_version": "0.1.0",
  "finished_at": "2026-11-10T12:14:56Z",
  "source": "/abs/server",
  "summary": { "deleted_chunks": 100, "size_before": 1122304, "size_after": 561152 },
  "forced": ["T-1", "L-1"]
}
```

### `wgc.failure/1`

```json
{
  "schema": "wgc.failure/1",
  "tool_version": "0.1.0",
  "stage": "verify",
  "error": "verification failed",
  "items": [
    { "check": "V-6", "path": "world/region/r.0.0.mca", "detail": "chunk 5 differs" },
    { "check": "V-4", "path": "world/region/r.0.1.mca", "detail": "size 8192, expected 4096" }
  ]
}
```

| Key | Value |
|---|---|
| `stage` | `write`, `verify`, `record` |
| `error` | Human-readable message with no guaranteed format |
| `items[]` | Verification failures; `check` is `V-1`..`V-6` |

## Compatibility

- An incompatible change (removing or renaming a key, changing a value's type) bumps the number in `schema`.
- Adding a key is a compatible change and does not bump the number. Readers ignore unknown keys.
- Golden tests pin every document byte for byte: `scan_json_golden`, `gc_json_golden` and `lock_status_json` in `crates/core/src/report/tests.rs`, the `*_json_golden` tests in `crates/core/src/output/record.rs`, and `verify_json_golden` in `crates/cli/src/main.rs`. The examples above are excerpts from them. Changing a shape requires updating both the tests and this document ([ADR-0012](adr/0012-pin-json-output-with-golden-tests.md)).
