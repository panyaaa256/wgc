# ワールドの構成の認識

レイアウトの判定は TMCJapan/sekai（`crates/world/src/tree/hostworld/`）と同じ結果にする。

## 判定

### ワールドのディレクトリ

`d/region/`、`d/entities/`、`d/poi/` のどれかに、解釈できる `r.<x>.<z>.mca` が1つ以上あれば、`d` はワールドのディレクトリ。`d` がなければ「ではない」。読めなければエラー（その他の I/O）。

### level-name

ルートの `server.properties` で、前後の空白を除いて `level-name=` で始まる行のうち、使える値を持つ最初の行の値（前後の空白を除く）。値が空・`.`・`..`・`/` か `\` を含む行は飛ばす（sekai と同じ）。ファイルがない・読めない、使える行がない場合は「指定なし」。

### Bukkit の候補 `<base>`

- `level-name` があれば、それだけ。
- なければ、ルート直下のディレクトリのうち、`dimensions/` を持つもの、または `<名前>_nether`／`<名前>_the_end` の兄弟を持つもの（`dimensions`、`DIM-1`、`DIM1` を除く）。`world` を先頭に、残りは辞書順。

### 順序（最初に一致したもの）

1. 各 `<base>` について:
   1. `<base>/dimensions/minecraft/{overworld,the_nether,the_end}` のどれかがワールドのディレクトリ → **Bukkit（26.1 以降）**
   2. `<base>/`、`<base>_nether/DIM-1/`、`<base>_the_end/DIM1/` のどれか → **Bukkit（26.1 より前）**
2. `dimensions/minecraft/{overworld,the_nether,the_end}` のどれか → **バニラ（26.1 以降）**
3. ルート、`DIM-1/`、`DIM1/` のどれか → **バニラ（26.1 より前）**
4. どれでもない → 「ワールドではない」

### 指定の誤りの検出

| ID | 条件 | 失敗 |
|---|---|---|
| W-1 | Bukkit（26.1 以降）と判定したが、ルートに `bukkit.yml` がない（Bukkit 系のサーバーは必ず作る）。または、Bukkit（26.1 より前）と判定したが、`<base>/DIM-1` か `<base>/DIM1` がワールドのディレクトリで、`<base>_nether/DIM-1` も `<base>_the_end/DIM1` もそうでない | バニラのサーバーらしい（正しい指定として `<root>/<base>` を示す） |
| W-2 | バニラと判定したが、ルートに `paper-world.yml` がある、またはルートの親に `bukkit.yml` か `spigot.yml` がある | Bukkit のワールドフォルダらしい |

## ディメンションマップ

| レイアウト | ディメンションフォルダ |
|---|---|
| Bukkit（26.1 以降） | `<base>/dimensions/<ns>/<name>` のすべて |
| Bukkit（26.1 より前） | `<base>/`、`<base>_nether/DIM-1/`、`<base>_the_end/DIM1/`、および兄弟 `<base>_<何か>/`（`_nether`、`_the_end` 以外）の中の `dimensions/<ns>/<name>` のすべて |
| バニラ（26.1 以降） | `dimensions/<ns>/<name>` のすべて |
| バニラ（26.1 より前） | ルート、`DIM-1/`、`DIM1/`、および `dimensions/<ns>/<name>`（あれば） |

- ID は `<ns>:<name>`。旧レイアウトの3つは `minecraft:overworld`、`minecraft:the_nether`、`minecraft:the_end`。Bukkit の兄弟フォルダの名前からは ID を決めない。
- Bukkit の兄弟フォルダに同じ ID のディメンションがあれば、エラー（同じ ID のディメンション）。
- シンボリックリンクは辿る。
- マップにない場所のリージョンファイルは探さない（出力では他のファイルと同じくそのままコピーされる）。

## 認識の結果

認識の結果として、次のものを決める。

- ルート（入力のパス）とレイアウトの種類。JSON での名前は `vanilla_legacy`（バニラ 26.1 より前）、`vanilla`（バニラ 26.1 以降）、`bukkit_legacy`（Bukkit 26.1 より前）、`bukkit`（Bukkit 26.1 以降）。
- `<base>`（Bukkit のときだけ）。
- メインのワールドフォルダ: `level.dat` とプレイヤーデータの場所。
- ワールドフォルダの一覧: ロックと出力の単位。パスの昇順に並べる。
- サーバーのルート（Bukkit のときだけ。ルートと同じ）。
- `level.dat` の DataVersion。
- ディメンションの一覧: ID の昇順に並べる。各ディメンションは、ID、ディメンションフォルダ、それを含むワールドフォルダ（`session.lock` の場所）を持つ。

| レイアウト | メインのワールドフォルダ | ディメンションを含むワールドフォルダ |
|---|---|---|
| バニラ | ルート | ルート |
| Bukkit（26.1 以降） | `<base>/` | `<base>/` |
| Bukkit（26.1 より前） | `<base>/` | そのディメンションを含む `<base>`、`<base>_nether`、`<base>_the_end`、または兄弟のフォルダ |

## リージョン三つ組

- 各ディメンションの三つ組は、`region/`、`entities/`、`poi/` にあるファイルの名前から得た座標の和集合からなり、座標の昇順に並ぶ。サイズ 0 のファイルも含める。
- 同じ種別のフォルダに同じ座標を指す2つの名前（`r.0.0.mca` と `r.00.00.mca`）があれば、エラー（同じ座標の2つの名前）。
- 各種別のフォルダにある `.mcc` は、名前が示すチャンクの位置で、同じ種別の三つ組のエントリに対応づける。同じ位置の `.mcc` が2つの名前で存在する場合も、エラー（同じ座標の2つの名前）。

## level.dat

`<メインのワールドフォルダ>/level.dat`（gzip NBT）。ない・読めない場合はエラー。

| フィールド | 用途 |
|---|---|
| `Data.DataVersion`（Int） | ワールドのバージョン |
| `Data.Player` | T-3。DataVersion < 4772 のシングルプレイヤーのホスト。4772 以降は `Data.singleplayer_uuid` に変わり、本体は他のプレイヤーと同じくプレイヤーデータの場所にある |

## ゲームルール

T-2 でモブのスポーンの設定を読む。キーがなければ既定値（有効）とする。

| DataVersion | 場所 | キーと型 |
|---|---|---|
| < 4658 | `level.dat` の `Data.GameRules` | `doMobSpawning`、String（`"true"`／`"false"`） |
| 4658 〜 4771 | `level.dat` の `Data.game_rules` | `minecraft:spawn_mobs`、Byte（0／1） |
| ≥ 4772 | `<メインのワールドフォルダ>/data/minecraft/game_rules.dat`（gzip NBT。ルートに `DataVersion` と `data`） | `data` の下の `minecraft:spawn_mobs`、Byte |

Bukkit 系ではゲームルールがワールドごとにある。

| レイアウト | 読む場所 |
|---|---|
| Bukkit（26.1 より前） | 各ワールドフォルダの `level.dat`（上の表の形式） |
| Bukkit（26.1 以降） | 各ディメンションフォルダの `data/minecraft/game_rules.dat` |

## プレイヤーデータ

| DataVersion | 場所 |
|---|---|
| < 4772 | `<メインのワールドフォルダ>/playerdata/` |
| ≥ 4772 | `<メインのワールドフォルダ>/players/data/` |

| パターン | 意味 |
|---|---|
| `<uuid>.dat` | 本体（gzip NBT） |
| `<uuid>.dat_old` | ゲームの予備 |
| `^[0-9a-f-]{36}-\d+\.dat$` | 保存の途中で残った一時ファイル |

## 強制読み込みチャンク

ファイルはどれも gzip NBT で、ルートに `DataVersion` と `data` がある。

| DataVersion | ファイル | 内容 |
|---|---|---|
| < 4297 | `<ディメンションフォルダ>/data/chunks.dat` | `data.Forced`: LongArray |
| 4297 〜 4771 | 同上 | `data.tickets`: Compound の List |
| ≥ 4772 | `<ディメンションフォルダ>/data/minecraft/chunk_tickets.dat` | 同上 |

- `data.tickets` の各要素は `type`（String）、`level`（Int）、`ticks_left`（Long）、`chunk_pos`。強制読み込みチャンクは `type` が `minecraft:forced` のもの（`minecraft:portal` などは無視する）。
- `chunk_pos` と `Forced` の値は、IntArray なら `[x, z]`、Long なら下位 32 ビットが x、上位 32 ビットが z。形式はタグの型で判断する。
- ファイルがなければ、強制読み込みチャンクはない。
- 読めない、または知らないバージョンなら「確認できない」（trust の T-5 で扱う）。

## Paper の設定ファイル

| ファイル | 場所 |
|---|---|
| ワールドごと（Bukkit 26.1 より前） | `<ワールドフォルダ>/paper-world.yml` |
| ワールドごと（Bukkit 26.1 以降） | `<ディメンションフォルダ>/paper-world.yml` |
| サーバー全体の既定値 | `<サーバーのルート>/config/paper-world-defaults.yml` |
| Paper 1.19 より前 | `<サーバーのルート>/paper.yml` の `world-settings.<ワールドフォルダ名>` と `world-settings.default`。`config/paper-world-defaults.yml` がなく `paper.yml` があるときだけ使う（1.19 で移行される） |

## 区別する失敗

| 失敗 | 意味 |
|---|---|
| ワールドではない | 判定の順序のどれにも一致しない |
| バニラのサーバーらしい（W-1） | Bukkit のサーバーのルートとして判定したが、バニラのサーバーのルートらしい。正しい指定として `<root>/<base>` を示す |
| Bukkit のワールドフォルダらしい（W-2） | バニラのワールドとして判定したが、Bukkit 系のサーバーのワールドフォルダらしい |
| 同じ座標の2つの名前 | 同じ種別のフォルダに、同じ座標を指す2つのファイル名がある（`.mcc` を含む） |
| 同じ ID のディメンション | Bukkit の兄弟フォルダに、同じ ID のディメンションがある |
| `level.dat` がない | メインのワールドフォルダに `level.dat` がない |
| `level.dat` を読めない | `level.dat` を gzip NBT として読めない |
| その他の I/O | 上記以外の読み込みの失敗 |

## 受け入れ条件

- 4種類のレイアウトで sekai と同じ判定になる。
- `level-name` が `../x`、空、`/abs` なら「指定なし」になる。
- 同じ座標を指す2つの名前でエラーになる。W-1、W-2 が検出される。
- Bukkit（26.1 より前）のデータパックのディメンションの ID が、中の `dimensions/<ns>/<name>` から決まる。
- 強制読み込みチャンクの3つの形式（`Forced`、`tickets` の Long、`tickets` の IntArray）を読め、`minecraft:forced` 以外のチケットを無視する。ファイルがなければ「なし」、壊れていれば「確認できない」になる。
- ゲームルールの3つの形式でモブのスポーンの設定を読め、キーがなければ有効として扱う。
