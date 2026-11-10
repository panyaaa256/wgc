# 診断（P6）

実装先: `core::report`（レポートの型と JSON）。端末への出力は `wgc`（P8）。

`scan` は、`--dimension` なしの Plan と信用の確認の結果から、`ScanReport` を組み立てる。独自の判定はしない。`gc` の見積もりと結果も、同じ型を使う。

## 使用中のワールド

読み始める前に `SessionLocks::try_acquire_all`（lock.md）を試し、取れたら読み終わるまで保持する。失敗したら `Warning::LockUnavailable` を出して続行する。

## 項目

### 全体

- レイアウト（`plan.layout` の種類、`base`、ワールドフォルダ）
- `level.dat` の DataVersion
- ロックの状態: `acquired`、`in_use`（このプロセスが持っている場合を含む）、`missing`、`not_writable`、`error`
- 信用の確認の全項目
- 出力しない他のファイル（プレイヤーデータの一時ファイル）、本体のない `.dat_old`
- 固定の注意文の ID

### ディメンションごと

- 種別ごとのサイズ（`.mcc` を含む）
- region のチャンク数と、古い形式のチャンク数
- InhabitedTime の分布（下表）
- GC の見積もり: 削除するチャンク数、保護したチャンク数（`forceload`、`structure_ref`）、種別ごとの出力後のサイズ、`NoEntriesLeft` のファイル
- 整合性: 構造エラー（ファイル、ID、インデックス）、内容エラー（位置、種類）、そのままコピーの三つ組
- `Orphan` のエントリ数（種別ごと）、サイズ 0 のファイル
- 古い地形との境界

### InhabitedTime の区分（tick）

| 区分 | 範囲 |
|---|---|
| `zero` | 0 |
| `missing` | タグなし、型の違い |
| `negative` | 負の値 |
| `unreadable` | 内容エラー |
| `lt_1s` | 1〜19 |
| `lt_10s` | 20〜199 |
| `lt_1m` | 200〜1199 |
| `lt_10m` | 1200〜11999 |
| `lt_1h` | 12000〜71999 |
| `lt_10h` | 72000〜719999 |
| `ge_10h` | 720000 以上 |

## JSON

types.md「レポートと JSON」の規則に従う。一覧は省略しない。並び順は Plan と同じ。互換性のない変更では `schema` の番号を上げる。

```json
{
  "schema": "wgc.scan/1",
  "tool_version": "0.1.0",
  "layout": { "kind": "bukkit_legacy", "base": "world", "world_folders": ["world", "world_nether", "world_the_end"] },
  "world_data_version": 4325,
  "lock": "acquired",
  "trust": [
    { "id": "T-1", "status": "pass" },
    { "id": "T-3", "status": "fail", "details": [{ "kind": "player_in_zero_chunk", "uuid": "…", "dimension": "minecraft:overworld", "pos": [3, -2] }] },
    { "id": "T-5", "status": "unknown", "reason": { "kind": "forced_chunks_unknown", "dimensions": ["minecraft:the_nether"] } }
  ],
  "other_files": { "omitted": [{ "path": "world/playerdata/…-123.dat", "reason": "player_temp_file" }], "dat_old_without_dat": [] },
  "notices": ["pregenerated_chunks"],
  "dimensions": [
    {
      "id": "minecraft:overworld",
      "dir": "world",
      "size": { "region": 0, "entities": 0, "poi": 0 },
      "chunks": { "total": 0, "older_data_version": 0 },
      "inhabited_time": { "zero": 0, "missing": 0, "negative": 0, "unreadable": 0, "lt_1s": 0, "lt_10s": 0, "lt_1m": 0, "lt_10m": 0, "lt_1h": 0, "lt_10h": 0, "ge_10h": 0 },
      "gc": { "delete": 0, "protected": { "forceload": 0, "structure_ref": 0 }, "size_after": { "region": 0, "entities": 0, "poi": 0 }, "emptied_files": [] },
      "integrity": { "structural": [{ "path": "world/region/r.0.0.mca", "id": "S-7", "index": 12 }], "content": [], "verbatim_triples": [] },
      "orphans": { "entities": 0, "poi": 0 },
      "empty_files": [],
      "boundary_warnings": [{ "chunks": 0, "min": [0, 0], "max": [0, 0] }]
    }
  ]
}
```

- `layout.kind`: `vanilla_legacy`、`vanilla`、`bukkit_legacy`、`bukkit`
- `trust[].status`: `pass`、`fail`、`unknown`、`not_applicable`
- `details` と `reason` は `TrustDetail`／`UnknownReason` を `kind` つきでシリアライズしたもの。
- `integrity.content[]` は `{ "pos": [x, z], "kind": "decompress" | "nbt", "detail": "…" }`、`verbatim_triples[]` はリージョン座標 `[x, z]`。
- 入力のパスの外にあるパス（ワールドフォルダを指定したときのサーバーの設定など）は、そのまま出す。

### `gc` の JSON

`"schema": "wgc.gc/1"`。上の全項目に次を加える。

| キー | 内容 |
|---|---|
| `mode` | `estimate`（`--out` なし）か `output` |
| `out` | 出力先（`output` のとき）。正規化した絶対パス |
| `forced` | `--force` で無視した確認の ID |
| `dimensions_targeted` | `--dimension` の値。指定がなければ `null` |

## ターミナル出力

人向けの表示は標準出力に出す（`--json` のときは標準エラーに出し、`--quiet` なら出さない）。

1. レイアウト、DataVersion、ロック
2. 信用の確認（不合格と「確認できない」を目立たせ、`gc --out` で止まることと、`--force` で続行できることを出す）。「確認できない」の理由は常に出す。不合格になった対象（ファイル、ワールドフォルダ、プレイヤーの位置）の一覧は `--verbose` のときだけ出す
3. ディメンションの表: サイズ、チャンク数、削除数、出力後のサイズ（「ディメンションの表」）
4. 合計: 現在、出力後、減る量
5. 整合性の問題の一覧（`--verbose` のときだけ）
6. clean の対象の一覧（`--verbose` のときだけ）
7. 古い形式のチャンクがあれば、公式の最適化を勧める
8. 古い地形との境界の一覧（`--verbose` のときだけ）
9. 固定の注意文

`gc` の見積もりでは、1〜6、8、9 を出す。各一覧は先頭 10 件までにし、残りは件数だけ出す。`--verbose` がないときに出さなかった一覧があれば、`--verbose` で見られることを出す。

### ディメンションの表

出力先が端末で、広い表がその幅に収まるときは広い表にし、そうでなければ狭い形にする。どちらも列の幅は中身に合わせる。

- 広い表: 3、4 の列に、整合性の問題の数、空ファイルの数、本体のない entities／poi のエントリの数、古い地形との境界の数を足す。合計の行は各列を足す。プレイヤーデータの一時ファイルはディメンションに属さないので、`--verbose` がなければ表の下に件数を出す。
- 狭い形: 3、4 だけを出す。`--verbose` がなければ、表の下に 5、6、8 の件数を1行にまとめて出す。

どちらも、件数が 0 の項目は出さない。

関数: `run_scan(layout, observer, cancel) -> Result<ScanReport, ReportError>`、`scan_report(plan, trust, lock)`、`gc_report(scan, out, forced, plan)`。`ReportError` は `NonUtf8Path` と `Plan(PlanError)`。

## 受け入れ条件

- InhabitedTime の各区分の境界値（-1、0、1、19、20、…、720000）が正しく分類される。
- JSON がスキーマに従い、同じ入力からバイト単位で同じ JSON ができる。
- 構造エラーのある三つ組が `integrity` に出る。
- ロックを取れない場合に警告して続行し、`lock` に状態が出る。
- UTF-8 でないパスがあると、レポートの作成がエラーになる。
