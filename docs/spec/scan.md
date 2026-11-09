# 診断（P6）

> 前提:
> - [plan.md](plan.md) の「Plan のモデル」
> - [trust.md](trust.md) の「結果のモデル」
>
> 実装先: `wgc-core` の `report` モジュール（レポートのモデルと JSON）。端末への出力は `wgc` クレート（P8）が行う。

## 目的

ワールドを読み取りだけで診断し、結果を端末向けの要約と JSON で出す。`scan` は、`gc` の Plan（モード `Gc`、全ディメンション）と、信用の確認の結果から、レポートを組み立てる。レポートのために独自の判定はしない。

## 起動中のワールド

`scan` は書き込まないので、ワールドが起動中でも実行できる。ただし、起動中は読んでいる途中でファイルが変わり、結果が不正確になりうる。

- `session.lock` のロックを試み（ロックの関数は P7 で実装する。[output.md](output.md) の「ロック」）、取れた場合は読み終わるまで保持する。
- 取れなかった場合は、警告「ワールドが使用中のため、結果が不正確な場合があります」を出して続行する。
- P6 の時点でロックの関数がなければ、ロックなしで実装し、P7 で接続する。

## レポートの項目

### 全体

| 項目 | 内容 |
|---|---|
| レイアウト | 4種類のどれか、`<base>`（Bukkit のとき）、ワールドフォルダの一覧 |
| ワールドの DataVersion | `level.dat` の値 |
| ロック | 取れたか |
| 信用の確認 | `TrustReport` の全項目 |
| 不明なディメンション | world-layout.md の W-3 |
| その他のファイル | 出力しないもの（プレイヤーデータの一時ファイル）の一覧。`.dat_old` の異常 |
| 注意 | 固定の注意文（[cli.md](cli.md) の「固定の注意文」）の ID |

### ディメンションごと

| 項目 | 内容 |
|---|---|
| 容量 | 種別ごとのファイルサイズの合計（`.mcc` を含む） |
| チャンク数 | region のチャンクの総数 |
| DataVersion の分布 | ワールドの DataVersion より古いチャンクの数（公式の最適化を勧める根拠） |
| InhabitedTime の分布 | 下の区分ごとのチャンク数 |
| GC の見積もり | 削除するチャンク数、保護したチャンク数（理由ごと）、種別ごとの出力後のサイズ |
| 空になるリージョンファイル | `NoEntriesLeft` になるファイルの一覧 |
| 断片化 | 断片化の合計 |
| 整合性 | 構造エラー（ファイル、ID、内容）、内容エラー（チャンクの位置、種類）、そのままコピーになる三つ組 |
| 本体のないデータ | `Orphan` で削除する entities／poi のエントリの数（種別ごと） |
| サイズ 0 のファイル | 一覧 |
| 報告のみ | 位置の不一致、型の違い、参照されていない外部ファイル |
| 古い地形との境界 | まとまりごとのチャンク数と範囲 |

### InhabitedTime の区分

単位は tick（20 tick = 1 秒）。

| 区分 | 範囲 |
|---|---|
| `zero` | 0 |
| `missing` | タグがない、または型が違う |
| `unreadable` | 内容エラー |
| `lt_1s` | 1 〜 19 |
| `lt_10s` | 20 〜 199 |
| `lt_1m` | 200 〜 1199 |
| `lt_10m` | 1200 〜 11999 |
| `lt_1h` | 12000 〜 71999 |
| `lt_10h` | 72000 〜 719999 |
| `ge_10h` | 720000 以上 |

## JSON（`--json <file>`）

- スキーマの識別子: `"schema": "wgc.scan/1"`。互換性のない変更をしたら番号を上げる。
- キーは snake_case。サイズはバイト数（整数）。チャンクの位置は `[x, z]`。
- 一覧は省略しない（端末向けの要約と違い、すべて出す）。並び順は Plan と同じ。

```json
{
  "schema": "wgc.scan/1",
  "tool_version": "0.1.0",
  "layout": { "kind": "bukkit_legacy", "base": "world", "world_folders": ["world", "world_nether", "world_the_end"] },
  "world_data_version": 4325,
  "lock": { "acquired": true },
  "trust": [
    { "id": "T1", "status": "pass" },
    { "id": "T3", "status": "fail", "details": ["…"], "override_flag": "--allow-zero-at-player" }
  ],
  "unknown_dimensions": [],
  "other_files": { "omitted": [{ "path": "world/playerdata/…-123.dat", "reason": "player_temp_file" }], "dat_old_without_dat": [] },
  "notices": ["pregenerated_chunks", "boundary_general"],
  "dimensions": [
    {
      "id": "minecraft:overworld",
      "dir": "world",
      "size": { "region": 0, "entities": 0, "poi": 0 },
      "chunks": { "total": 0, "older_data_version": 0 },
      "inhabited_time": { "zero": 0, "missing": 0, "unreadable": 0, "lt_1s": 0, "lt_10s": 0, "lt_1m": 0, "lt_10m": 0, "lt_1h": 0, "lt_10h": 0, "ge_10h": 0 },
      "gc": {
        "delete": 0,
        "protected": { "forceload": 0, "spawn": 0, "structure_ref": 0 },
        "size_after": { "region": 0, "entities": 0, "poi": 0 },
        "emptied_files": []
      },
      "fragmentation": 0,
      "integrity": { "structural": [], "content": [], "verbatim_triples": [] },
      "orphans": { "entities": 0, "poi": 0 },
      "empty_files": [],
      "reports": { "pos_mismatch": [], "type_mismatch": [], "unreferenced_external": [] },
      "boundary_warnings": [{ "chunks": 0, "min": [0, 0], "max": [0, 0] }]
    }
  ]
}
```

`trust[].status` は `pass`、`fail`、`unknown`、`not_applicable` のどれか。`layout.kind` は `vanilla_legacy`、`vanilla`、`bukkit_legacy`、`bukkit` のどれか。

## ターミナル出力

端末向けの要約は、次の順に出す。各一覧は先頭の 10 件までにし、残りは件数だけを出す（すべて見るには `--json`）。

1. レイアウト、ワールドの DataVersion、ロックの状態
2. 信用の確認（不合格と「確認できない」を目立たせる。続行用のフラグも出す）
3. ディメンションごとの1行の要約: 容量、チャンク数、削除するチャンク数、出力後の容量
4. 合計: 現在の容量、出力後の容量、減る量
5. 整合性の問題（あれば）
6. clean の対象（サイズ 0 のファイル、プレイヤーデータの一時ファイル、本体のないデータ）
7. 古い形式のチャンクがあれば、公式の最適化を勧める1行
8. 古い地形との境界（あれば）
9. 固定の注意文

`gc`／`clean` の見積もり（`--out` なし）でも、同じ形式で、1〜4、5、6、8、9 を出す。

## 受け入れ条件

- 合成したワールドで、InhabitedTime の各区分の境界値（0、1、19、20、…、720000）が正しい区分に入る。
- JSON が上のスキーマに従い、同じ入力から同じ JSON（バイト単位で一致）ができる。
- 構造エラーのある三つ組が、整合性の項目と `verbatim_triples` に出る。
- ロックを取れない場合に警告を出して続行する。
