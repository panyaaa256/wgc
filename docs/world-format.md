# World format reference

This document lists the external facts about Minecraft Java Edition, Bukkit, and Paper storage formats that wgc relies on: region files, chunk NBT, world layouts, world data files, and `session.lock`. It also defines how wgc classifies malformed input (S-, C-, and W- IDs), since those IDs appear in wgc's output. wgc's deletion and keep rules and its trust checks are in [gc-rules.md](gc-rules.md). Internals are in [ARCHITECTURE.md](../ARCHITECTURE.md), and usage is in [README.md](../README.md). Version labels such as `DV_CHUNK_ROOT` are defined in [Version reference](#version-reference).

## Coordinates and dimension IDs

- **Packed chunk position.** A chunk position packed into one Long stores x in the low 32 bits and z in the high 32 bits, both signed. Structure references, forced chunks, and `chunk_pos` use this encoding.
- **Region and index.** Chunk `(x, z)` belongs to region `(x >> 5, z >> 5)` at index `(x & 31) + (z & 31) * 32` (0 to 1023). In the other direction, index `i` of region `(rx, rz)` is chunk `(rx * 32 + (i & 31), rz * 32 + (i >> 5))`.
- **Dimension ID.** A dimension ID has the form `<ns>:<name>`. An ID without a namespace, such as `overworld`, gets `minecraft:`. wgc rejects malformed IDs.

## Region files

### File names

| File | Pattern |
|---|---|
| Region file | `r.<rx>.<rz>.mca`. Each coordinate matches `-?[0-9]+`. Leading zeros are allowed (`r.00.00.mca`), and `+` is not. |
| External chunk file | `c.<cx>.<cz>.mcc` in the same folder. `<cx>` and `<cz>` are absolute chunk coordinates; wgc parses them with the same `-?[0-9]+` rule. |

- A file that does not match the pattern is not a region file.
- A region coordinate whose chunks do not all have coordinates that fit in a signed 32-bit integer (`rx * 32` through `rx * 32 + 31`) is not a region file.
- One region kind folder (`region/`, `entities/`, or `poi/`) must not contain two names for the same coordinates, such as `r.0.0.mca` and `r.00.00.mca`. The same rule applies to two `.mcc` names for the same chunk. wgc reports either case as an error.
- Before 1.16, Paper's "Allow Saving of Oversized Chunks" option kept the region format unchanged and wrote the extra data to other files in the same folder (`*.oversized.nbt` and others). wgc treats these files like any other file and copies them unchanged.

### Header (8192 bytes)

| Bytes | Content |
|---|---|
| 0 to 4095 | Location table: 1024 entries of 4 bytes. The high 3 bytes (big-endian) hold the start sector, and the low byte holds the sector count. One sector is 4096 bytes. |
| 4096 to 8191 | Timestamps: 1024 × u32 (big-endian). |

A location entry of four zero bytes means there is no chunk at that index.

### Payload

The payload begins at byte `start sector × 4096`:

| Field | Size |
|---|---|
| Length `L` (big-endian; includes the compression byte) | u32 |
| Compression method | u8 |
| Data | `L - 1` bytes |

The payload is these `4 + L` bytes.

### Compression methods

| Value | Meaning |
|---|---|
| 1 | gzip |
| 2 | zlib |
| 3 | Uncompressed |
| 4 | LZ4 (see [LZ4](#lz4)) |
| 127 | Custom. wgc does not decompress it (C-1). |
| Any of the above + 128 | The data is in the external `.mcc` file. The file contains only the compressed data, using the method in the low 7 bits. The payload in the region file normally has `L = 1`. |
| Any other value | C-1 |

### LZ4

Method 4 does not use the standard LZ4 frame format. It uses the block stream format of lz4-java's `LZ4BlockOutputStream`.

Each block starts with a 21-byte header:

| Field | Size |
|---|---|
| Magic `LZ4Block` | 8 |
| Token. The high 4 bits are the method (`0x10` uncompressed, `0x20` LZ4), and the low 4 bits are the compression level. | 1 |
| Compressed length | i32 LE |
| Original length | i32 LE |
| Checksum | i32 LE |

- The compressed data follows the header and is as long as the compressed length. When the method is LZ4, the data is in LZ4 block format.
- The checksum is the XXHash32 (seed `0x9747b28c`) of the decompressed data, masked with `& 0x0FFFFFFF`. A mismatch is C-1.
- The stream ends with a block whose original length is 0. Like lz4-java, wgc treats a stream without this end block as C-1.

### Structural errors

Structural errors apply to a whole file. wgc reports every structural error it finds. Each report names the region kind, the ID, and the chunk index. S-1 concerns the whole file and has no index. How wgc handles a region triple that has a structural error is described in [gc-rules.md](gc-rules.md).

| ID | Condition |
|---|---|
| S-1 | The file size is at least 1 and less than 8192. |
| S-2 | Only one of start sector and sector count is 0. |
| S-3 | The start sector is less than 2. |
| S-4 | `start sector × 4096 + 4 + L` is greater than the file size. This includes the case where the length field itself cannot be read. |
| S-5 | `L` is 0. |
| S-6 | `4 + L` is greater than `sector count × 4096`. |
| S-7 | The sector ranges of two chunks overlap. |
| S-8 | The compression method is 128 or greater, but the `.mcc` file does not exist. |

- The last sector of a file can be shorter than 4096 bytes. For this reason, the range check uses the actual length (S-4) instead of the sector count.
- A file of size 0 is not a structural error. wgc treats it as a file with no chunks.

### Content errors

Content errors apply to a single chunk. wgc keeps the chunk and copies its payload unchanged.

| ID | Condition |
|---|---|
| C-1 | The chunk cannot be decompressed. |
| C-2 | The chunk cannot be read as NBT (see [Chunk NBT](#chunk-nbt)). |

wgc distinguishes these C-1 failures:

| Failure | Meaning |
|---|---|
| Unsupported method | The method is 127 (custom) or an unknown value. |
| External file missing | The method points to an external file, but the external file has no data (decompressing an S-8 chunk). |
| Too short | The payload is shorter than 5 bytes. |
| Corrupt | The compressed data is corrupt. |
| Too large | The decompressed data is larger than 256 MiB. |
| LZ4 checksum mismatch | An LZ4 block's checksum does not match its decompressed data. |

### Writing

wgc writes a new region file from the set of chunks it keeps. The written file has this layout:

- Kept chunks are placed in ascending index order from sector 2, with no gaps.
- Each payload is written byte for byte as it was in the original file, then padded with zeros to the next 4096-byte boundary.
- The location table holds the new start sector and `ceil((4 + L) / 4096)`.
- A kept chunk keeps its original timestamp. Every other timestamp is 0.
- `.mcc` files are not part of the region file write.

The write fails if a chunk to keep is not in the original file, or if one chunk needs more than 255 sectors. Neither can happen for a file without structural errors.

## Chunk NBT

The input is the decompressed NBT of a `region` chunk: a named root Compound. wgc reads only these fields:

| Field | Type | DataVersion ≥ 2844 (`DV_CHUNK_ROOT`) | Earlier |
|---|---|---|---|
| `DataVersion` | Int | Root | Root (may be absent) |
| `InhabitedTime` | Long | Root | Under `Level` |
| `blending_data` | Compound | Root (only whether it is present) | Not read |
| Structure references | Compound of LongArray | `structures.References` | `Level.Structures.References` |

- wgc looks at both the root and `Level`. It uses the root value when DataVersion is 2844 or greater, and the `Level` value when DataVersion is less than 2844 or absent. A tag that is missing on the selected side is treated as absent.
- A tag of the wrong type is treated as absent. wgc does not report it as an error.
- `blending_data` counts only when DataVersion ≥ 2844.
- `Status`, `xPos`, and `zPos` are not read.
- If a key appears more than once, the last one wins, as in the game.
- Keys are compared as raw bytes, without checking the text encoding.
- Bytes after the root Compound are ignored, as in the game.
- A negative `InhabitedTime` is kept as is. How wgc treats it is described in [gc-rules.md](gc-rules.md).
- Each value in each LongArray under `References` is a [packed chunk position](#coordinates-and-dimension-ids). wgc ignores the structure IDs and collects all values into one set.
- wgc does not decompress `entities` or `poi` chunks. It only checks those files for structural errors.

When a chunk cannot be read as NBT, the result is C-2. wgc distinguishes these failures:

| Failure | Meaning |
|---|---|
| Unexpected end | The data ends in the middle of the NBT. |
| Unknown tag type | A tag type value that NBT does not define. |
| Invalid string | A string that cannot be decoded. |
| Root is not a Compound | The root tag is not a Compound. |
| Other | Anything else, such as an invalid length or nesting that is too deep. |

## World layouts

wgc detects layouts with the same results as TMCJapan/sekai (`crates/world/src/tree/hostworld/`).

### Detection

**World directory.** `d` is a world directory if at least one of `d/region/`, `d/entities/`, or `d/poi/` contains a valid `r.<x>.<z>.mca`. If `d` does not exist, it is not a world directory. If `d` cannot be read, the result is an I/O error.

**`level-name`.** wgc reads the root's `server.properties`. It takes the first line that starts with `level-name=` after trimming whitespace and that has a usable value, and it trims whitespace from that value. A line is skipped when its value is empty, is `.` or `..`, or contains `/` or `\`, as in sekai. If the file is missing or unreadable, or no line qualifies, `level-name` is unset.

**Bukkit base candidates (`<base>`).**

- If `level-name` is set, it is the only candidate.
- Otherwise, the candidates are the root's subdirectories that have a `dimensions/` folder or a sibling named `<name>_nether` or `<name>_the_end`. The directories `dimensions`, `DIM-1`, and `DIM1` are excluded. `world` comes first, and the rest are in lexicographic order.

**Order.** The first match wins:

1. For each `<base>`:
   1. If any of `<base>/dimensions/minecraft/{overworld,the_nether,the_end}` is a world directory, the layout is **Bukkit (26.1+)**.
   2. If any of `<base>/`, `<base>_nether/DIM-1/`, or `<base>_the_end/DIM1/` is a world directory, the layout is **Bukkit (pre-26.1)**.
2. If any of `dimensions/minecraft/{overworld,the_nether,the_end}` is a world directory, the layout is **Vanilla (26.1+)**.
3. If any of the root, `DIM-1/`, or `DIM1/` is a world directory, the layout is **Vanilla (pre-26.1)**.
4. Otherwise, the input is not a world.

### Dimension map

| Layout | Dimension folders |
|---|---|
| Bukkit (26.1+) | Every `<base>/dimensions/<ns>/<name>` |
| Bukkit (pre-26.1) | `<base>/`, `<base>_nether/DIM-1/`, `<base>_the_end/DIM1/`, and every `dimensions/<ns>/<name>` inside a sibling `<base>_<anything>/` (other than `_nether` and `_the_end`) |
| Vanilla (26.1+) | Every `dimensions/<ns>/<name>` |
| Vanilla (pre-26.1) | The root, `DIM-1/`, `DIM1/`, and any `dimensions/<ns>/<name>` |

- The ID of a dimension folder is `<ns>:<name>`. The three legacy folders are `minecraft:overworld`, `minecraft:the_nether`, and `minecraft:the_end`. The name of a Bukkit sibling folder does not determine an ID.
- wgc reports an error if two Bukkit sibling folders contain dimensions with the same ID.
- Symbolic links are followed.
- wgc does not look for region files outside the dimension map.

### World folders

A world folder is a folder that contains `level.dat`. The main world folder holds `level.dat` and player data. The world folder that contains a dimension holds that dimension's `session.lock`.

| Layout | Main world folder | World folder containing a dimension |
|---|---|---|
| Vanilla | The root | The root |
| Bukkit (26.1+) | `<base>/` | `<base>/` |
| Bukkit (pre-26.1) | `<base>/` | Whichever of `<base>`, `<base>_nether`, `<base>_the_end`, or a sibling folder contains the dimension |

### Misdirected paths

| ID | Condition | Failure |
|---|---|---|
| W-1 | The layout is Bukkit (26.1+), but the root has no `bukkit.yml` (Bukkit-based servers always create it). Or the layout is Bukkit (pre-26.1), and `<base>/DIM-1` or `<base>/DIM1` is a world directory, but neither `<base>_nether/DIM-1` nor `<base>_the_end/DIM1` is one. | The input looks like a vanilla server. wgc suggests `<root>/<base>` as the correct path. |
| W-2 | The layout is Vanilla, and the root has `paper-world.yml`, or the root's parent has `bukkit.yml` or `spigot.yml`. | The input looks like a world folder of a Bukkit-based server. |

wgc also distinguishes these failures: not a world, two names for the same coordinates, two dimensions with the same ID, missing `level.dat`, unreadable `level.dat`, and other I/O errors.

## World data files

All `.dat` files below are gzip-compressed NBT.

### level.dat

The file is `<main world folder>/level.dat`. wgc reports an error if it is missing or unreadable.

| Field | Use |
|---|---|
| `Data.DataVersion` (Int) | The world version |
| `Data.Player` | The singleplayer host when DataVersion < 4772 (used by T-3, see [gc-rules.md](gc-rules.md)). From 4772, this field is replaced by `Data.singleplayer_uuid`, and the host's data is stored with the other players' data. |

### Game rules

The mob spawning rule is used by T-2 (see [gc-rules.md](gc-rules.md)). A missing key means the default, which is enabled.

| DataVersion | Location | Key and type |
|---|---|---|
| < 4658 | `level.dat`, `Data.GameRules` | `doMobSpawning`, String (`"true"` / `"false"`) |
| 4658 to 4771 | `level.dat`, `Data.game_rules` | `minecraft:spawn_mobs`, Byte (0 / 1) |
| ≥ 4772 | `<main world folder>/data/minecraft/game_rules.dat` (root has `DataVersion` and `data`) | `minecraft:spawn_mobs` under `data`, Byte |

Bukkit-based servers store game rules for each world:

| Layout | Location |
|---|---|
| Bukkit (pre-26.1) | `level.dat` in each world folder (formats above) |
| Bukkit (26.1+) | `data/minecraft/game_rules.dat` in each dimension folder |

### Player data

| DataVersion | Folder |
|---|---|
| < 4772 | `<main world folder>/playerdata/` |
| ≥ 4772 | `<main world folder>/players/data/` |

| File name | Meaning |
|---|---|
| `<uuid>.dat` | Player data |
| `<uuid>.dat_old` | The game's backup |
| `^[0-9a-f-]{36}-\d+\.dat$` | Temporary file left by an interrupted save |

### Forced chunks

Each file's root has `DataVersion` and `data`.

| DataVersion | File | Content |
|---|---|---|
| < 4297 (`DV_TICKETS`) | `<dimension folder>/data/chunks.dat` | `data.Forced`: LongArray |
| 4297 to 4771 | Same as above | `data.tickets`: List of Compound |
| ≥ 4772 | `<dimension folder>/data/minecraft/chunk_tickets.dat` | Same as above |

- Each `data.tickets` entry has `type` (String), `level` (Int), `ticks_left` (Long), and `chunk_pos`. A forced chunk is a ticket whose `type` is `minecraft:forced`. Other types, such as `minecraft:portal`, are ignored.
- A `chunk_pos` or `Forced` value is either an IntArray `[x, z]` or a Long that stores x in the low 32 bits and z in the high 32 bits. The tag type tells which.
- If the file does not exist, there are no forced chunks. If it is unreadable or has an unknown version, the forced chunks cannot be verified (used by T-5, see [gc-rules.md](gc-rules.md)).

### Paper config files

These files are used by T-1 (see [gc-rules.md](gc-rules.md)).

| Scope | Location |
|---|---|
| Per world (Bukkit pre-26.1) | `<world folder>/paper-world.yml` |
| Per world (Bukkit 26.1+) | `<dimension folder>/paper-world.yml` |
| Server-wide defaults | `<server root>/config/paper-world-defaults.yml` |
| Before Paper 1.19 | `world-settings.<world folder name>` and `world-settings.default` in `<server root>/paper.yml`. wgc reads them only when `config/paper-world-defaults.yml` is missing and `paper.yml` exists (Paper 1.19 migrates them). |

The server root is the input root (Bukkit layouts only).

## session.lock

- At startup, the game creates `session.lock`, writes `☃`, calls fsync, and then locks the file. Because of this, the file's modification time always changes when the game starts (used by `wgc verify`, see [README.md](../README.md)).
- The game locks the file with Java's `FileChannel.tryLock`. On Unix this is an `fcntl` record lock, and on Windows it is `LockFileEx`.
- On Linux, `flock` does not conflict with `fcntl` locks, so `flock` gives no mutual exclusion with the game.
- A process loses its `fcntl` record locks on a file when it closes any descriptor for that file, even a different one.
- Games older than 20w14a (`DV_SESSION_LOCK_OS`) do not lock the file, so wgc cannot detect whether they are running.

## Version reference

Other documents refer to versions by these labels. Where the format can be told apart by tag type or by the presence of a tag, wgc uses that instead of a DataVersion check.

| Label | Version | DataVersion | Meaning |
|---|---|---|---|
| `DV_SESSION_LOCK_OS` | 20w14a (1.16 snapshot) | 2524 | From this version, `session.lock` holds an OS lock. |
| `DV_CHUNK_ROOT` | 21w43a | 2844 | Chunk NBT drops `Level`, and its fields move to the root. |
| `DV_TICKETS` | 1.21.5 snapshot | 4297 | Forced chunks are saved as `tickets` instead of `Forced`. |
| `DV_1_21_5` | 1.21.5 | 4325 | Lower bound for T-2 (a Paper bug). |
| `DV_GAMERULE_REGISTRY` | Just before 25w44a | 4658 | `GameRules` in `level.dat` becomes `game_rules`, with new key and value formats. |
| `DV_FILES_26_1` | 26.1 snapshot | 4772 | File layout changes: dimensions, player data, data split out of `level.dat`, and namespaced `data/`. |
| `DV_MAX_KNOWN` | 26.3 | 5023 | The newest storage format wgc knows. It is updated for each release. A world whose `level.dat` `Data.DataVersion` is greater than this has an unknown version (T-4). |

For reference: 1.21.11 = 4671, 26.1 = 4786.

Chunks and `level.dat` from before 1.9 (DataVersion 169) may have no DataVersion. wgc treats them as the oldest format.

## Sources

The storage formats and version values were checked on 2026-11-09 against:

- The decompiled Minecraft 26.3 server jar (not obfuscated): `DataFixers`, the `FileFix` classes, `TicketStorage`, `GameRuleRegistryFix`, `DirectoryLock`, `SerializableChunkData`, `RegionFileVersion`.
- The official mappings of the 1.16 snapshots, which show when `DirectoryLock` was introduced.
- PaperMC/Paper (26.3, commit `b31b4b5`) and Issue #12827.
- lz4-java's `LZ4BlockOutputStream`.
