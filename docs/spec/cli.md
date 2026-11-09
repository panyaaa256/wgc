# CLI（P8）

実装先: `wgc`（`crates/cli`）。引数の解析は clap。

## コマンド

```
wgc scan   <path> [--json <file>]
wgc gc     <path> [--out <dir>] [--dim <id>]... [--force]
wgc verify <path> <out>
```

| オプション | 意味 |
|---|---|
| `scan --json <file>` | JSON を書く。`-` なら標準出力に書き、要約は標準エラーに出す |
| `gc --out <dir>` | 出力を書く。なければ見積もりだけ出し、何も書かない |
| `gc --dim <id>` | GC の削除をそのディメンションだけで行う（複数可）。`overworld`、`the_nether`、`the_end` は `minecraft:` を補う。存在しない ID はエラー |
| `gc --force` | 判定の信用に関する停止を無視する |

## 停止と `--force`

| 停止 | ID | `--force` |
|---|---|---|
| Paper の `fixed-chunk-inhabited-time` | T-1 | 続行できる |
| モブのスポーンが無効 | T-2 | 続行できる |
| プレイヤーのいるチャンクが 0 | T-3 | 続行できる |
| 知らないバージョン | T-4 | 続行できる |
| 強制読み込みチャンクの情報を読めない | T-5 | 続行できる |
| `session.lock` がない | L-1 | 続行できる |
| ワールドが使用中 | — | 続行できない |
| 出力先の不正 | — | 続行できない |
| 空き容量の不足 | — | 続行できない |
| 検証の失敗 | — | 続行できない |
| レイアウトの判定のエラー（W-1、W-2 など） | — | 続行できない |

`--force` で続行したら、無視した確認を ID と内容つきで目立つように表示する。

## 終了コード

| コード | 意味 |
|---|---|
| 0 | 成功 |
| 1 | 成功したが警告がある（`scan` と見積もりで信用の確認が不合格の場合を含む） |
| 2 | 引数の誤り |
| 3 | 停止した（何も書いていない） |
| 4 | エラー（I/O、レイアウトの判定など） |
| 5 | 検証の失敗（`gc` の出力の検証、`verify`） |

## 固定の注意文

| ID | 場面 | 文 |
|---|---|---|
| `pregenerated_chunks` | `scan`、`gc` | Pre-generated chunks are also deleted. On a pre-generated world, this undoes the pre-generation. |
| `boundary_general` | 削除するチャンクがあるとき | Deleted chunks are regenerated with the current version and world generation settings when they are next loaded. Terrain may not line up where it meets older chunks. |
| `no_trash` | `gc --out` の完了時 | The original world was not modified. There is no trash: once you delete the original world, it cannot be restored. Keep it until you have checked the new one. |
| `run_official_optimize` | 古い形式のチャンクがあるとき | N chunks are in an older format. Consider running the game's Optimize World (`--forceUpgrade`). |

## 出力

- 進捗は、端末のときだけ標準エラーに出す。
- 警告とエラーは標準エラーに出す。
- 停止したときは、理由と解決の方法（判定の信用に関する停止なら `--force`）を出す。
- メッセージは英語のみ。1か所にまとめる。

## 受け入れ条件

- 各コマンドの `--help` が出る。
- `--out` なしの `gc` が、何も書かない（ワールドの全ファイルの更新日時が変わらない）。
- 終了コードが表のとおりになる。
- `--dim` の省略形と、存在しない ID のエラー。
- `scan --json -` で、標準出力が JSON だけになる。
