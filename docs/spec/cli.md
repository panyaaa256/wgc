# CLI（P8）

実装先: `wgc`（`crates/cli`）。引数の解析は clap。メッセージは英語のみで、`messages` モジュールの1か所にまとめる。

## コマンド

```
wgc [-q | --quiet] [--no-color] <command>
wgc scan   <path> [--json] [--strict]
wgc gc     <path> [-o | --out <dir>] [--dimension <id>]... [-f | --force[=<ids>]] [--json] [--strict]
wgc verify <out> [--source <path>] [--json]
wgc --version
wgc --help
```

`<path>` は、バニラではワールドフォルダ、Bukkit 系ではサーバーのルートフォルダ。

| オプション | 意味 |
|---|---|
| `--json` | JSON（scan.md「JSON」）を標準出力に出す。人向けの表示は標準エラーに出す |
| `--strict` | 警告があれば終了コード 1 にする |
| `-o`, `--out <dir>` | 出力を書く。なければ見積もりだけ出し、何も書かない |
| `--dimension <id>` | GC の削除をそのディメンションだけで行う（複数可）。`--dim` を別名にする。`overworld`、`the_nether`、`the_end` は `minecraft:` を補う。存在しない ID は引数の誤り |
| `-f`, `--force[=<ids>]` | 判定の信用に関する停止を無視する。`--force` だけならすべて、`--force=T-3,T-5` なら指定した ID だけ。不明な ID は引数の誤り |
| `--source <path>` | `verify` で、記録された元のワールドの代わりに比べるパス |
| `-q`, `--quiet` | 進捗と人向けの要約を出さない（警告とエラーは出す） |
| `--no-color` | 色を使わない |

## 停止と `--force`

| 停止 | ID | `--force` |
|---|---|---|
| Paper の `fixed-chunk-inhabited-time` | T-1 | 続行できる |
| モブのスポーンが無効 | T-2 | 続行できる |
| プレイヤーのいるチャンクが 0 | T-3 | 続行できる |
| 知らないバージョン | T-4 | 続行できる |
| 強制読み込みチャンクの情報を読めない | T-5 | 続行できる（強制読み込みの保護なしで削除する） |
| `session.lock` がない | L-1 | 続行できる |
| ワールドが使用中 | — | 続行できない |
| 出力先の不正 | — | 続行できない |
| 空き容量の不足 | — | 続行できない |
| 検証の失敗 | — | 続行できない |
| レイアウトの判定のエラー | — | 続行できない |

`--force` で続行したら、無視した確認を ID と内容つきで目立つように表示する。T-5 を無視したときは、強制読み込みの保護がないことも表示する。

## 終了コード

| コード | 意味 |
|---|---|
| 0 | 成功（警告があっても。`--force` で続行して成功した場合も） |
| 1 | `--strict` のときに警告があった |
| 2 | 引数の誤り |
| 3 | 停止した（何も書いていない） |
| 4 | エラー |
| 5 | 検証の失敗 |
| 130 | 中断された（Ctrl-C） |

| エラー | コード |
|---|---|
| `LayoutError`（すべて） | 4 |
| `LockError::InUse`、`NotWritable` | 3 |
| `LockError::Io` | 4 |
| `OutputError::OutputNotEmpty`、`InvalidOutputPath`、`OutputOverlapsSource`、`Stopped`、`InsufficientSpace` | 3 |
| `OutputError::Verification` | 5 |
| `OutputError::Io`、`Plan(PlanError::Io)` | 4 |
| `OutputError::Cancelled`、`PlanError::Cancelled` | 130 |
| `VerifyError::Incomplete`、`SourceMismatch`、`Changed`、`InUse` | 5 |
| `VerifyError::Io` | 4 |

Ctrl-C を受けたら中断の旗を立て、core が戻るのを待って 130 で終わる。2回目の Ctrl-C ではすぐに終わる。

## 出力

- 人向けの表示は標準出力。`--json` のときは標準出力を JSON だけにし、人向けの表示は標準エラーに出す。
- 進捗は、標準エラーが端末で、`--quiet` でないときだけ標準エラーに出す。
- 警告とエラーは標準エラーに出す。
- 色は、出力先が端末で、`NO_COLOR` が未設定か空で、`TERM` が `dumb` でなく、`--no-color` がないときだけ使う。
- 停止したときは、理由と解決の方法（判定の信用に関する停止なら `--force`）を出す。
- `gc --out` の完了時と `verify` の成功時に、書き戻しの手順（output.md）を出す。`complete.json` の `forced` が空でなければ、`This output was created with --force (ignored checks: ...)` を目立つように出す。

### `wgc verify` の JSON

```json
{ "schema": "wgc.verify/1", "result": "ready", "source": "/abs/path", "changed": [], "forced": [] }
```

`result`: `ready`、`incomplete`、`source_mismatch`、`changed`、`in_use`。

## メッセージ

| 場面 | 文 |
|---|---|
| W-1 | `This looks like a vanilla server. Pass the world folder instead: wgc gc <suggested>` |
| W-2 | `This looks like a world folder of a Bukkit-based server. Pass the server root folder instead.` |
| ロック: 使用中 | `The world is in use. Stop the server or game and try again.` |
| ロック: 書き込めない | `Cannot open session.lock for writing. Run wgc as the user that runs the server.` |
| scan でロックを取れない | `The world is in use. Results may be inaccurate.` |
| verify: 不完全 | `The output is incomplete.` |
| verify: 変更あり | `The world has changed since the output was created. Swapping it in would lose those changes.` |
| verify: 使用中 | `The world is in use.` |
| verify: 成功 | `Ready to swap.` |

### 固定の注意文

| ID | 場面 | 文 |
|---|---|---|
| `pregenerated_chunks` | `scan`、`gc` | Pre-generated chunks are also deleted. On a pre-generated world, this undoes the pre-generation. |
| `boundary_general` | 削除するチャンクがあるとき | Deleted chunks are regenerated with the current version and world generation settings when they are next loaded. Terrain may not line up where it meets older chunks. |
| `no_trash` | `gc --out` の完了時 | The original world was not modified. There is no trash: once you delete the original world, it cannot be restored. Keep it until you have checked the new one. |
| `run_official_optimize` | 古い形式のチャンクがあるとき | N chunks are in an older format. Consider running the game's Optimize World (`--forceUpgrade`). |

## ヘルプ

各コマンドの `--help` に、用途、引数、例を載せる。`gc --help` には「`--out` がなければ何も書かない」ことと、次の例を含める。

```
wgc gc ./world                  # estimate only
wgc gc ./world -o ./world-gc    # write the result to ./world-gc
wgc verify ./world-gc           # check before swapping
```

## 受け入れ条件

- 各コマンドの `--help` と `--version` が出る。
- `--out` なしの `gc` が、何も書かない（ワールドの全ファイルの更新日時が変わらない）。
- 終了コードが表のとおりになる。警告だけなら 0、`--strict` で 1。
- `--dimension` の省略形と別名、存在しない ID のエラー。`--force=T-3` で T-5 の停止が残る。
- `--json` で、標準出力が JSON だけになる。
- `NO_COLOR=1` で色が出ない。
- Ctrl-C で 130 で終わり、出力先に `complete.json` がない。
