# ワールドの構成の認識（P3）

実装先: `core::layout`（NBT を読む部分は `core::nbt`）。レイアウトの判定は TMCJapan/sekai（`crates/world/src/tree/hostworld/`）と同じ結果にする。

## 判定

### ワールドのディレクトリ

`d/region/`、`d/entities/`、`d/poi/` のどれかに、解釈できる `r.<x>.<z>.mca` が1つ以上あれば、`d` はワールドのディレクトリ。`d` がなければ「ではない」。読めなければエラー。

### level-name

ルートの `server.properties` の、コメントでない最初の `level-name=` の値（前後の空白を除く）。ファイルがない・読めない、値が空・`.`・`..`・`/` か `\` を含む場合は「指定なし」。

### Bukkit の候補 `<base>`

- `level-name` があれば、それだけ。
- なければ、ルート直下のディレクトリのうち、`dimensions/` を持つもの、または `<名前>_nether`／`<名前>_the_end` の兄弟を持つもの（`dimensions`、`DIM-1`、`DIM1` を除く）。`world` を先頭に、残りは辞書順。

### 順序（最初に一致したもの）

1. 各 `<base>` について:
   1. `<base>/dimensions/minecraft/{overworld,the_nether,the_end}` のどれかがワールドのディレクトリ → **Bukkit（26.1 以降）**
   2. `<base>/`、`<base>_nether/DIM-1/`、`<base>_the_end/DIM1/` のどれか → **Bukkit（26.1 より前）**
2. `dimensions/minecraft/{overworld,the_nether,the_end}` のどれか → **バニラ（26.1 以降）**
3. ルート、`DIM-1/`、`DIM1/` のどれか → **バニラ（26.1 より前）**
4. どれでもない → エラー

### 指定の誤りの検出

| ID | 条件 | エラーメッセージ |
|---|---|---|
| W-1 | Bukkit（26.1 より前）と判定したが、`<base>/DIM-1` か `<base>/DIM1` がワールドのディレクトリで、`<base>_nether/DIM-1` も `<base>_the_end/DIM1` もそうでない | `This looks like a vanilla server. Pass the world folder instead: wgc gc /srv/mc/world`（実行したコマンドのパスを `<root>/<base>` に置き換えて出す） |
| W-2 | バニラと判定したが、ルートに `paper-world.yml` がある、またはルートの親に `bukkit.yml` か `spigot.yml` がある | `This looks like a world folder of a Bukkit-based server. Pass the server root folder instead.` |

## ディメンションマップ

| レイアウト | ディメンションフォルダ |
|---|---|
| Bukkit（26.1 以降） | `<base>/dimensions/<ns>/<name>` のすべて |
| Bukkit（26.1 より前） | `<base>/`、`<base>_nether/DIM-1/`、`<base>_the_end/DIM1/`、および兄弟 `<base>_<何か>/`（`_nether`、`_the_end` 以外）の中の `dimensions/<ns>/<name>` のすべて |
| バニラ（26.1 以降） | `dimensions/<ns>/<name>` のすべて |
| バニラ（26.1 より前） | ルート、`DIM-1/`、`DIM1/`、および `dimensions/<ns>/<name>`（あれば） |

- ID は `<ns>:<name>`。旧レイアウトの3つは `minecraft:overworld`、`minecraft:the_nether`、`minecraft:the_end`。Bukkit の兄弟フォルダの名前からは ID を決めない。
- シンボリックリンクは辿る。
- マップにない場所のリージョンファイルは探さない（P7 でそのままコピーされる）。

```rust
pub struct Dimension {
    pub id: String,
    pub dir: PathBuf,           // ディメンションフォルダ
    pub world_folder: PathBuf,  // session.lock の場所
}
```

| レイアウト | `world_folder` |
|---|---|
| バニラ | ルート |
| Bukkit（26.1 以降） | `<base>/` |
| Bukkit（26.1 より前） | そのディメンションを含む `<base>`、`<base>_nether`、`<base>_the_end`、または兄弟のフォルダ |

ワールド全体として次も持つ。

- `main_world_folder`: `level.dat` とプレイヤーデータの場所。バニラはルート、Bukkit は `<base>/`。
- `world_folders`: すべてのワールドフォルダ。ロックと出力の単位。
- `server_root`: Bukkit のときだけ、ルート。

## リージョン三つ組の列挙

各ディメンションで:

1. `region/`、`entities/`、`poi/` のファイルを列挙し、名前から座標を得る。
2. 同じ種別のフォルダに同じ座標を指す2つの名前（`r.0.0.mca` と `r.00.00.mca`）があれば、エラー。
3. 3種別の座標の和集合を、座標の昇順で三つ組にする。サイズ 0 のファイルも含める。
4. 各種別のフォルダの `.mcc` も三つ組に渡す。

## level.dat

`main_world_folder/level.dat`（gzip NBT）。ない・読めない場合はエラー。

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
| ≥ 4772 | `main_world_folder/data/minecraft/game_rules.dat`（gzip NBT。ルートに `DataVersion` と `data`） | `data` の下の `minecraft:spawn_mobs`、Byte |

Bukkit 系ではゲームルールがワールドごとにある。

| レイアウト | 読む場所 |
|---|---|
| Bukkit（26.1 より前） | 各ワールドフォルダの `level.dat`（上の表の形式） |
| Bukkit（26.1 以降） | 各ディメンションフォルダの `data/minecraft/game_rules.dat` |

## プレイヤーデータ

| DataVersion | 場所 |
|---|---|
| < 4772 | `main_world_folder/playerdata/` |
| ≥ 4772 | `main_world_folder/players/data/` |

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
- 読めない、または知らないバージョンなら「確認できない」（P4 で `protection_unknown`）。

## Paper の設定ファイル

| ファイル | 場所 |
|---|---|
| ワールドごと（Bukkit 26.1 より前） | `<world_folder>/paper-world.yml` |
| ワールドごと（Bukkit 26.1 以降） | `<ディメンションフォルダ>/paper-world.yml` |
| サーバー全体の既定値 | `<server_root>/config/paper-world-defaults.yml` |

## 受け入れ条件

- 4種類のレイアウトで sekai と同じ判定になる。
- `level-name` が `../x`、空、`/abs` なら「指定なし」になる。
- 同じ座標を指す2つの名前でエラーになる。W-1、W-2 が検出される。
- Bukkit（26.1 より前）のデータパックのディメンションの ID が、中の `dimensions/<ns>/<name>` から決まる。
- 強制読み込みチャンクの3つの形式（`Forced`、`tickets` の Long、`tickets` の IntArray）を読め、`minecraft:forced` 以外のチケットを無視する。ファイルがなければ「なし」、壊れていれば「確認できない」になる。
- ゲームルールの3つの形式で `spawn_mobs` を読め、キーがなければ有効として扱う。
