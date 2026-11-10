# wgc

*Read this in [日本語](README_ja.md).*

Delete chunks that no player has ever been near from a Minecraft Java Edition world.

wgc is a chunk garbage collector for Anvil worlds (`.mca`). It removes chunks
whose `InhabitedTime` is 0, together with their entities and POI data, and
writes the result to a new folder. The original world is never modified: you
check the output with `wgc verify` and swap it in yourself.

- Only chunks with `InhabitedTime = 0` are deleted. Anything wgc cannot judge is kept.
- Kept chunks are written byte for byte; chunk data is never re-encoded.
- The output is verified against the original world before it is reported as complete.

## Installation

Prebuilt binaries are attached to each
[GitHub Release](https://github.com/panyaaa256/wgc/releases)
(`wgc-<target triple>.tar.gz`). Alternatively, with a Rust toolchain installed:

```sh
cargo install --git https://github.com/panyaaa256/wgc --locked
```

The installed binary is named `wgc`. wgc depends on a git crate, so it is not
published on crates.io.

## Usage

```sh
# Diagnose the world without writing anything
wgc scan ./world

# Estimate how much would be deleted (nothing is written without --out)
wgc gc ./world

# Write the result to a new or empty folder
wgc gc ./world -o ./world-gc

# Check that the output can be swapped in
wgc verify ./world-gc
```

Pass the world folder for a vanilla server or single-player world, or the
server root folder for Bukkit-based servers (Spigot, Paper, and others). wgc
warns when it looks like you passed the wrong one.

Stop the server or game before running `wgc gc --out`. wgc takes the world's
`session.lock` and refuses to run while the world is in use.

### Swapping in the result

When `gc --out` finishes and when `verify` succeeds, wgc prints these steps with
the actual paths:

1. Stop the game or server.
2. Make sure `wgc verify <out>` succeeds.
3. Rename `<path>` to `<path>.wgc-bak`.
4. Move `<out>` to `<path>`.
5. Start the game or server and check the world.
6. If everything looks fine, delete `<path>.wgc-bak`. This cannot be undone, so keep it for a while.
7. You may delete `<path>/.wgc/`.

On Bukkit-based servers, do steps 3 and 4 for each world folder and leave `.wgc/` behind.

### Options

| Option | Commands | Meaning |
|---|---|---|
| `-o`, `--out <dir>` | `gc` | Write the result. Without it, only an estimate is printed |
| `--dimension <id>` | `gc` | Only delete chunks in this dimension (repeatable). Alias: `--dim`. `overworld`, `the_nether` and `the_end` get the `minecraft:` prefix |
| `-f`, `--force[=<ids>]` | `gc` | Continue past failed trust checks: all of them, or only the listed IDs (e.g. `--force=T-3,T-5`) |
| `--source <path>` | `verify` | Compare against this path instead of the recorded original world |
| `--json` | `scan`, `gc`, `verify` | Print JSON on stdout. The human-readable summary goes to stderr |
| `--strict` | `scan`, `gc` | Exit with code 1 when there are warnings |
| `-q`, `--quiet` | all | Do not print progress or the summary. Warnings and errors are still printed |
| `--no-color` | all | Do not use colors. `NO_COLOR` is also respected |

Run `wgc <command> --help` for details.

## What gets deleted

A chunk is deleted only if its `InhabitedTime` is 0 and none of the following applies:

- It is a force-loaded chunk, or within 2 chunks of one.
- A kept chunk references a structure that starts in it.
- Its region file is damaged. Damaged region files are copied unchanged.
- Its dimension is excluded with `--dimension`.

Chunks whose `InhabitedTime` is missing, negative, or unreadable are kept.

`gc` also drops files that the game does not need: empty region files,
temporary player data files, and entities/POI data whose chunk no longer exists.

## Trust checks

The rule above assumes that `InhabitedTime` grows whenever a player is near a
chunk. Before writing, `gc --out` checks that this holds and stops if it may not.

| ID | Stops when |
|---|---|
| T-1 | Paper's `fixed-chunk-inhabited-time` is set, so the value is frozen |
| T-2 | Mob spawning is disabled on Paper 1.21.5 or later, where `InhabitedTime` then stops growing ([PaperMC/Paper#12827](https://github.com/PaperMC/Paper/issues/12827)) |
| T-3 | A player is standing in a chunk with `InhabitedTime = 0` |
| T-4 | The world was saved by a newer version than wgc knows |
| T-5 | Force-loaded chunks cannot be read |
| L-1 | The world has no `session.lock` |

If you know the check does not apply to your world, pass `--force` (or
`--force=<ids>`) to continue. wgc never continues when the world is in use, the
output folder is invalid, disk space is insufficient, or verification fails.

## Things to know

- Pre-generated chunks are also deleted. On a pre-generated world, this undoes the pre-generation.
- Deleted chunks are regenerated with the current version and world generation settings when they are next loaded. Terrain may not line up where it meets older chunks.
- There is no trash. Once you delete the original world, it cannot be restored.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success, including with warnings or `--force` |
| 1 | There were warnings and `--strict` was given |
| 2 | Invalid arguments |
| 3 | Stopped before writing anything |
| 4 | Error |
| 5 | Verification failed |
| 130 | Interrupted (Ctrl-C) |

## Compatibility

- Minecraft Java Edition worlds in the Anvil format, up to 26.3 (DataVersion 5023)
- Vanilla and Bukkit-based server layouts, including the file layout introduced in 26.1

## Documentation

The specification (in Japanese) is in [`docs/spec.md`](docs/spec.md), with
details per topic under [`docs/spec/`](docs/spec/).

## License

[MIT](LICENSE)
