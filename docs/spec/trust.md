# InhabitedTime の信用の確認（P5）

実装先: `core::trust`。

GC は「InhabitedTime が 0 なら、スペクテイター以外のプレイヤーが半径 128 ブロック以内に来たことがない」ことを前提にする（バニラでは、エンティティが処理される範囲で、128 ブロック以内にプレイヤーがいるチャンクに毎 tick 加算される）。この前提を確かめる。

| ID | 確認 | 対象 |
|---|---|---|
| T-1 | Paper の設定 | Bukkit のレイアウト |
| T-2 | モブのスポーン | Bukkit のレイアウトで DataVersion ≥ `DV_1_21_5` |
| T-3 | プレイヤーの位置 | すべて |
| T-4 | バージョン | すべて |
| T-5 | 強制読み込みチャンクの情報 | すべて |

## T-1: Paper の設定

`fixed-chunk-inhabited-time` が 0 以上だと、全チャンクの値が固定される。

ワールドごとの `paper-world.yml` と既定値の `paper-world-defaults.yml`（場所は world-layout「Paper の設定ファイル」）の `chunks.fixed-chunk-inhabited-time` を読む（YAML ライブラリを使う）。

| ワールドごとの値 | 判定 |
|---|---|
| 0 以上 | 不合格 |
| `-1` | 合格 |
| `default`、キーなし、ファイルなし | 既定値のファイルで判定（0 以上なら不合格、それ以外は合格） |

YAML を読めない、型が想定外なら「確認できない」。不合格のときは、ファイルと値を表示する。

## T-2: モブのスポーン

Paper 1.21.5 以降では、ゲームルール `spawn_mobs`（旧 `doMobSpawning`）が無効だと InhabitedTime が加算されない（PaperMC/Paper#12827。2026-11-09 時点で未修正で、Paper 26.3 のコードにも残っている）。

各ワールドのゲームルール（world-layout「ゲームルール」）で、モブのスポーンが無効なら不合格。形式を読めない、または知らないバージョンなら「確認できない」。Spigot など Paper 以外ではこの不具合は起きないが、ファイルからは区別できないので `--force` で続行する。

## T-3: プレイヤーの位置

対象: プレイヤーデータの場所の `<uuid>.dat` と、`level.dat` の `Data.Player`（DataVersion < 4772 のときだけある）。

| フィールド | 型 | 内容 |
|---|---|---|
| `Pos` | Double の List（3 要素） | x、y、z |
| `Dimension` | Int または String | Int は `0`、`-1`、`1` を `minecraft:overworld`、`minecraft:the_nether`、`minecraft:the_end` に対応させる。形式はタグの型で判断する |
| `playerGameType` | Int | `3` はスペクテイター |

各プレイヤーについて、次の場合は飛ばす: スペクテイター、`Dimension` がマップにない、フィールドを読めない、チャンク `(floor(x / 16), floor(z / 16))` が存在しない、InhabitedTime がタグなし。残りで InhabitedTime が 0 なら「該当」。

| 状況 | 判定 |
|---|---|
| 該当が 1 人以上 | 不合格。該当するプレイヤー（UUID、ディメンション、チャンクの位置）をすべて表示する |
| 該当がなく、確認できたプレイヤーが 1 人以上 | 合格 |
| 確認できたプレイヤーが 0 人 | 確認できない |

## T-4: バージョン

`level.dat` の DataVersion > `DV_MAX_KNOWN` なら「確認できない」。

## T-5: 強制読み込みチャンクの情報

Plan の `protection_unknown` が空でなければ「確認できない」。

## 結果のモデル

```rust
pub struct TrustReport {
    pub checks: Vec<CheckResult>, // ID の順
}

pub struct CheckResult {
    pub id: CheckId,              // T1..T5
    pub status: CheckStatus,
}

pub enum CheckStatus {
    Pass,
    Fail { details: Vec<String> },
    Unknown { reason: String },
    NotApplicable,
}
```

## 判定の使い方

- `scan` と見積もり: すべて表示し、中止しない。
- `gc --out`: 不合格か「確認できない」が1つでもあれば、何も書かずに中止する。`--force` があれば続行し、無視した確認を ID と内容つきで目立つように表示し、`complete.json` の `forced` に記録する。

## 受け入れ条件

- T-1: ワールドごとの値（`0`、`-1`、`default`、キーなし）と既定値の組み合わせが表のとおりに判定される。
- T-2: Bukkit で DataVersion ≥ 4325 かつモブのスポーンが無効なら不合格、バニラなら対象外。
- T-3: スペクテイター、存在しないチャンク、マップにないディメンションが飛ばされる。0 のチャンクにいるプレイヤーが 1 人で不合格、0 人で「確認できない」。`Dimension` の Int と String、`Data.Player` を扱える。
- `gc --out` が、不合格か「確認できない」で中止し、`--force` で続行して `forced` に記録する。
