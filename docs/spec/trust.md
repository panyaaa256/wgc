# InhabitedTime の信用の確認（P5）

実装先: `core::trust`。

GC は「InhabitedTime が 0 なら、スペクテイター以外のプレイヤーが半径 128 ブロック以内に来たことがない」ことを前提にする（バニラでは、エンティティが処理される範囲で、128 ブロック以内にプレイヤーがいるチャンクに毎 tick 加算される）。この前提を確かめる。

```rust
pub fn check_trust(plan: &Plan) -> TrustReport;
```

読めないファイルはエラーにせず、その確認を「確認できない」にする。

## 確認の一覧と範囲

GC の対象のディメンション（`plan.gc_dims`、指定がなければ全ディメンション）に関係する範囲だけを見る。

| ID | 確認 | 対象 | 範囲 |
|---|---|---|---|
| T-1 | Paper の設定 | Bukkit のレイアウト | 対象のディメンションを含むワールド |
| T-2 | モブのスポーン | Bukkit のレイアウトで DataVersion ≥ `DV_1_21_5` | 対象のディメンションを含むワールド |
| T-3 | プレイヤーの位置 | すべて | 対象のディメンションにいるプレイヤー |
| T-4 | バージョン | すべて | ワールド全体 |
| T-5 | 強制読み込みチャンクの情報 | すべて | 対象のディメンション |

## T-1: Paper の設定

`fixed-chunk-inhabited-time` が 0 以上だと、全チャンクの値が固定される。

ワールドごとの `paper-world.yml` と既定値の `paper-world-defaults.yml`（場所は world-layout「Paper の設定ファイル」）の `chunks.fixed-chunk-inhabited-time` を、`yaml-rust2` で読む。

| ワールドごとの値 | 判定 |
|---|---|
| 0 以上 | 不合格 |
| `-1` | 合格 |
| `default`、キーなし、ファイルなし | 既定値のファイルで判定（0 以上なら不合格、それ以外は合格） |

YAML を読めない、型が想定外なら「確認できない」。

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

各プレイヤーについて、次の場合は飛ばす: スペクテイター、`Dimension` がマップにないか GC の対象外、フィールドを読めない、チャンク `(floor(x / 16), floor(z / 16))` が存在しない、InhabitedTime が `Value` 以外。残りで `Value(0)` なら「該当」。

| 状況 | 判定 |
|---|---|
| 該当が 1 人以上 | 不合格（該当するプレイヤーをすべて記録する） |
| 該当がなく、確認できたプレイヤーが 1 人以上 | 合格 |
| 確認できたプレイヤーが 0 人 | 確認できない |

## T-4: バージョン

`level.dat` の DataVersion > `DV_MAX_KNOWN` なら「確認できない」。

## T-5: 強制読み込みチャンクの情報

Plan の `protection_unknown` に対象のディメンションがあれば「確認できない」。

## 結果のモデル

```rust
pub struct TrustReport {
    pub checks: Vec<CheckResult>,              // T-1..T-5 の順
}

pub struct CheckResult {
    pub id: GuardId,                           // T1..T5
    pub status: CheckStatus,
}

pub enum CheckStatus {
    Pass,
    Fail(Vec<TrustDetail>),
    Unknown(UnknownReason),
    NotApplicable,
}

pub enum TrustDetail {
    PaperFixedInhabitedTime { file: PathBuf, value: i64 },
    MobSpawningDisabled { world_folder: PathBuf },
    PlayerInZeroChunk { uuid: String, dimension: DimensionId, pos: ChunkPos },
}

pub enum UnknownReason {
    UnreadableFile { path: PathBuf },
    UnexpectedValue { path: PathBuf, key: String },
    UnknownVersion(DataVersion),
    NoPlayersChecked,
    ForcedChunksUnknown(BTreeSet<DimensionId>),
}
```

## 判定の使い方

- `scan` と見積もり: すべて表示し、中止しない。
- `gc --out`: 不合格か「確認できない」が1つでもあれば、何も書かずに中止する（`OutputError::Stopped`）。`--force` で無視を許された ID だけは続行し、`Warning::Forced(id)` を出して `complete.json` の `forced` に記録する。T-5 を無視すると強制読み込みの保護がないまま削除する。

## 受け入れ条件

- T-1: ワールドごとの値（`0`、`-1`、`default`、キーなし）と既定値の組み合わせが表のとおりに判定される。
- T-2: Bukkit で DataVersion ≥ 4325 かつモブのスポーンが無効なら不合格、バニラなら対象外。
- T-3: スペクテイター、存在しないチャンク、マップにないディメンション、負の値が飛ばされる。0 のチャンクにいるプレイヤーが 1 人で不合格、0 人で「確認できない」。`Dimension` の Int と String、`Data.Player` を扱える。
- `--dimension minecraft:the_end` のとき、オーバーワールドのプレイヤーや強制読み込みの情報では止まらない。
- `gc --out` が、不合格か「確認できない」で中止し、許された ID だけ続行して `forced` に記録する。
