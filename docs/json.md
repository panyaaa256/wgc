# JSON の出力

スクリプトや AI エージェント向けの、機械が読むための約束。

- `scan`、`gc`、`verify` は `--json` を受け付ける。`--json` のとき、標準出力には JSON の文書を1つだけ出し、人向けの表示は標準エラーに出す。JSON に色は付けない。
- `gc --out` は、出力先の `<out>/.wgc/` に記録の JSON を書く。
- 終了コードは JSON の有無で変わらない（spec/cli.md「終了コード」）。

## 文書の一覧

| `schema` | 出すもの |
|---|---|
| `wgc.scan/1` | `wgc scan --json` |
| `wgc.gc/1` | `wgc gc --json`（見積もりと出力の両方） |
| `wgc.verify/1` | `wgc verify --json` |
| `wgc.source-state/1` | `<out>/.wgc/source-state.json` |
| `wgc.complete/1` | `<out>/.wgc/complete.json` |
| `wgc.failure/1` | `<out>/.wgc/failure.json` |

## 共通の規則

- キーと列挙の値は snake_case。
- パスは入力のパスからの相対パスで、区切りは `/`。入力のパスそのものは `"."`。入力のパスの外にあるパス（ワールドフォルダを指定したときのサーバーの設定など）は、そのまま出す。
- 例外として `source` と `out` は正規化した絶対パス。
- UTF-8 でないパスがあれば、JSON を作らずにエラーにする。
- 一覧は省略しない。同じ入力からは、バイト単位で同じ JSON を出す。一覧の並び順は、ディメンションの ID、リージョン座標、ファイル内のインデックス、パスの順。
- 確認の ID は `T-1`〜`T-5`、`L-1`、構造エラーの ID は `S-1`〜`S-8`、検証の ID は `V-1`〜`V-6` の形の文字列。

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

| キー | 値 |
|---|---|
| `layout.kind` | `vanilla_legacy`、`vanilla`、`bukkit_legacy`、`bukkit`（spec/world-layout.md「認識の結果」） |
| `layout.base` | Bukkit のときの `<base>`。バニラでは `null` |
| `world_data_version` | `level.dat` の DataVersion。ないときは `null` |
| `lock` | `acquired`（取れた）、`in_use`（使用中、またはこのプロセスが保持中）、`missing`（`session.lock` がない）、`not_writable`（書き込めない）、`error`（その他の I/O） |
| `trust[].status` | `pass`、`fail`、`unknown`、`not_applicable` |
| `trust[].details` | `fail` のときだけ。下の「不合格の対象」の配列 |
| `trust[].reason` | `unknown` のときだけ。下の「確認できない理由」 |
| `other_files.omitted[].reason` | `empty_file`（サイズ 0）、`no_entries_left`（残すエントリがない）、`player_temp_file`（プレイヤーデータの一時ファイル） |
| `other_files.dat_old_without_dat` | 本体の `<uuid>.dat` がない `<uuid>.dat_old` のパス |
| `notices` | 固定の注意文の ID（spec/cli.md「固定の注意文」） |
| `dimensions[].chunks.older_data_version` | ワールドより古い形式のチャンクの数 |
| `dimensions[].inhabited_time` | InhabitedTime の区分ごとのチャンク数（spec/scan.md「InhabitedTime の区分」） |
| `dimensions[].gc.emptied_files` | 残すエントリがなくなって出力しないファイル |
| `dimensions[].integrity.structural[]` | 構造エラー。`index` は S-1 では `null` |
| `dimensions[].integrity.content[]` | 内容エラー。`{ "pos": [x, z], "kind": "decompress" \| "nbt", "detail": "…" }`。`detail` は人向けの説明で、形を約束しない |
| `dimensions[].integrity.verbatim_triples[]` | そのままコピーするリージョン三つ組の座標 `[x, z]` |
| `dimensions[].orphans` | 本体のない entities／poi のエントリの数 |
| `dimensions[].boundary_warnings[]` | 古い地形との境界のまとまり。チャンク数と範囲 |

### 不合格の対象（`details[]`）

| `kind` | フィールド | 確認 |
|---|---|---|
| `paper_fixed_inhabited_time` | `file`、`value` | T-1 |
| `mob_spawning_disabled` | `world_folder` | T-2 |
| `player_in_zero_chunk` | `uuid`、`dimension`、`pos` | T-3 |

### 確認できない理由（`reason`）

| `kind` | フィールド |
|---|---|
| `unreadable_file` | `path` |
| `unexpected_value` | `path`、`key` |
| `unknown_version` | `data_version` |
| `no_players_checked` | なし |
| `forced_chunks_unknown` | `dimensions` |

## `wgc.gc/1`

`wgc.scan/1` の全項目の後ろに次を加える。

```json
{ "schema": "wgc.gc/1", "…": "wgc.scan/1 と同じ項目", "mode": "output", "out": "/abs/out", "forced": ["T-1", "T-5", "L-1"], "dimensions_targeted": ["minecraft:overworld"] }
```

| キー | 値 |
|---|---|
| `mode` | `estimate`（`--out` なし）か `output` |
| `out` | 出力先の絶対パス。`estimate` では `null` |
| `forced` | `--force` で無視した確認の ID |
| `dimensions_targeted` | `--dimension` の値。指定がなければ `null` |

## `wgc.verify/1`

```json
{ "schema": "wgc.verify/1", "result": "ready", "source": "/abs/server", "changed": [], "forced": ["T-1", "L-1"] }
{ "schema": "wgc.verify/1", "result": "changed", "source": null, "changed": ["world/level.dat", "world/region/r.0.0.mca"], "forced": [] }
```

| キー | 値 |
|---|---|
| `result` | `ready`、`incomplete`、`source_mismatch`、`changed`、`in_use`、`error`（読み込みの失敗） |
| `source` | 元のワールドの絶対パス。`ready` 以外では `null` |
| `changed` | `changed` のとき、変わったファイルのパス |
| `forced` | `complete.json` の `forced` |

## `<out>/.wgc/` の記録

### `wgc.source-state/1`

各ワールドフォルダの下の全ファイル（`session.lock` を含む）の、相対パス、サイズ、更新日時（ナノ秒）。

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

出力が完了したことを示す（spec/output.md「実行の保証」）。

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

| キー | 値 |
|---|---|
| `stage` | `write`（書き出し）、`verify`（検証）、`record`（記録） |
| `error` | 人向けの説明。形を約束しない |
| `items[]` | 検証の失敗。`check` は `V-1`〜`V-6` |

## 互換性

- 互換性のない変更（キーの削除や改名、値の型の変更）では、`schema` の番号を上げる。
- キーの追加は互換性のある変更として扱い、番号を上げない。読む側は知らないキーを無視する。
- golden test が、すべての文書の形をバイト単位で固定している（`crates/core/src/report/tests.rs` の `scan_json_golden`・`gc_json_golden`・`lock_status_json`、`crates/core/src/output/record.rs` の `*_json_golden`、`crates/cli/src/main.rs` の `verify_json_golden`）。上の例はそこからの抜粋。形を変えるときは、テストとこの文書の両方を直す。
