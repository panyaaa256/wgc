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
| W-1 | Bukkit（26.1 より前）と判定したが、`<base>/DIM-1` か `<base>/DIM1` がワールドのディレクトリで、`<base>_nether/DIM-1` も `<base>_the_end/DIM1` もそうでない | 「バニラのサーバーでは、ワールドフォルダを指定してください: `wgc gc /srv/mc/world`」（実行したコマンドのパスを `<root>/<base>` に置き換えて出す） |
| W-2 | バニラと判定したが、ルートに `paper-world.yml` がある、またはルートの親に `bukkit.yml` か `spigot.yml` がある | 「Bukkit 系のサーバーでは、サーバーのルートを指定してください」 |

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
    pub world_folder: PathBuf,  // session.lock と paper-world.yml の場所
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
| `Data.GameRules` | T-2。旧形式はキー `doMobSpawning`、値は String（`"true"`／`"false"`）。要調査: 最近のバージョンで名前・型・場所が変わった可能性 |
| `Data.Player` | T-3（シングルプレイヤーのホスト） |

Bukkit（26.1 より前）では、ゲームルールはワールドフォルダごとの `level.dat` から読む（T-2）。それ以外は `main_world_folder` のものを使う。

## プレイヤーデータ

場所: 26.1 より前は `main_world_folder/playerdata/`。26.1 以降は要調査。

| パターン | 意味 |
|---|---|
| `<uuid>.dat` | 本体（gzip NBT） |
| `<uuid>.dat_old` | ゲームの予備 |
| `^[0-9a-f-]{36}-\d+\.dat$` | 保存の途中で残った一時ファイル |

## 強制読み込みチャンク

| バージョン | 場所と形式 |
|---|---|
| 1.14 〜（要調査: いつまでか） | `<ディメンションフォルダ>/data/chunks.dat`（gzip NBT）の `data.Forced`（LongArray。下位 32 ビットが x、上位 32 ビットが z） |
| それ以降 | 要調査 |

- ファイルがなければ、強制読み込みチャンクはない。
- 読めない、または知らないバージョンなら「確認できない」（P4 で `protection_unknown`）。

## Paper の設定ファイル

| ファイル | 場所 |
|---|---|
| ワールドごと | `<world_folder>/paper-world.yml` |
| サーバー全体の既定値 | `<server_root>/config/paper-world-defaults.yml` |

26.1 以降の場所は要調査。

## 受け入れ条件

- 4種類のレイアウトで sekai と同じ判定になる。
- `level-name` が `../x`、空、`/abs` なら「指定なし」になる。
- 同じ座標を指す2つの名前でエラーになる。W-1、W-2 が検出される。
- Bukkit（26.1 より前）のデータパックのディメンションの ID が、中の `dimensions/<ns>/<name>` から決まる。
- `chunks.dat` がなければ「なし」、壊れていれば「確認できない」になる。
