# 計画（P4）

> 前提:
> - [region-format.md](region-format.md) の「エラーの区分」（構造エラーと内容エラー）
> - [chunk-nbt.md](chunk-nbt.md) の「ChunkSummary」
> - [world-layout.md](world-layout.md) の「ディメンションマップ」「リージョン三つ組の列挙」「強制読み込みチャンク」「プレイヤーデータ」
>
> 実装先: `wgc-core` の `plan` モジュール。

## 目的

ワールドを読み、「どのチャンクを消し、どのファイルをどう出力するか」を決めた **Plan** を作る。Plan はただのデータで、ファイルには書き込まない。

- `scan`、`gc` の見積もり、`gc --out` の実行（P7）は、すべて同じ Plan を使う。
- 実行のときは、Plan を作り直す。過去の `scan` の出力を使い回さない。
- 同じ入力からは、同じ Plan ができる（並び順も含めて決定的にする）。

## 適用する規則

GC の規則と clean の規則を、常に両方適用する。

- `dims`（`--dim` の指定）があれば、GC の削除はそのディメンションだけで行う。clean の規則は、全ディメンションに適用する。

## 処理の流れ

### 段階 A: リージョン三つ組ごと（並列）

各リージョン三つ組について:

1. `region`、`entities`、`poi` の各ファイルを解析し、構造エラーを集める。
2. 3つのうちどれかに構造エラーがあれば、三つ組を **そのままコピー** とする（理由: どのファイルに、どの構造エラーがあるか）。
   - region に構造エラーがあると、entities／poi の対応が信用できない。entities／poi に構造エラーがあると、region のチャンクを消したときに、対応するエンティティや POI を消せない。どちらの場合も、三つ組全体を触らない。
3. region の各チャンクを展開して `ChunkSummary` を作る。失敗したチャンクは内容エラーとして記録する。
   - 三つ組がそのままコピーの場合も、読めるチャンクは要約する（構造物の参照を集めるため）。読めないものは無視する。

### 段階 B: 全体（逐次）

1. **削除候補** C: GC の対象のディメンションの、そのままコピーでない三つ組にある、`inhabited_time == Some(0)` のチャンク。
2. **強制読み込みによる保護** F: 各強制読み込みチャンクと、そこからチェビシェフ距離 2 以内のチャンク（同じディメンション）。強制読み込みチャンクの周りの 2 チャンクは、ブロックが処理される範囲と境界のチャンクで、流体やレッドストーンが入り込みうる。
3. **構造物の参照による保護** R: InhabitedTime が 0 でないチャンク（0 より大きい、判断できない、内容エラー、または三つ組がそのままコピー）の `structure_refs` の和集合（同じディメンション）。参照はリージョン三つ組をまたぐので、この段階は全体で行う。保護したチャンクの参照をさらにたどることはしない（1回だけ）。
4. 削除するチャンク D = C − F − R。

強制読み込みチャンクが「確認できない」場合、その保護は計算できない。Plan にその旨を記録する（`protection_unknown`）。`gc --out` は、[trust.md](trust.md) の T-5 として扱う。`scan` と見積もりでは、その保護がないものとして計算し、その旨を表示する。

## 判定の規則

### region のチャンク

削除するのは、次のすべてを満たすチャンクだけ。

- そのディメンションが GC の対象である
- 三つ組がそのままコピーでない
- 内容エラーがない
- `inhabited_time == Some(0)`
- F にも R にも含まれない

それ以外は残す。残す理由は、次の優先順位で1つ記録する。

| 優先 | 理由 | 意味 |
|---|---|---|
| 1 | `TripleVerbatim` | 三つ組がそのままコピー |
| 2 | `NotTargeted` | GC の対象外のディメンション |
| 3 | `ContentError` | 展開できない、NBT を読めない |
| 4 | `InhabitedUnknown` | InhabitedTime のタグがない、または型が違う |
| 5 | `Inhabited` | InhabitedTime が 0 より大きい |
| 6 | `Forceload` | 強制読み込みによる保護 |
| 7 | `StructureRef` | 構造物の参照による保護 |

### entities／poi のエントリ

三つ組がそのままコピーの場合は、すべて残す。そうでない場合、各エントリについて、同じ位置の region のチャンクを見る。

| region の同じ位置 | 判定 |
|---|---|
| チャンクがあり、残す | 残す |
| チャンクがあり、削除する | 削除する（理由: `GcDeleted`） |
| チャンクがない（region のファイルがない、サイズ 0、またはエントリが空） | 削除する（理由: `Orphan`。clean の規則） |

「region が読めない」場合は、三つ組がそのままコピーになっているので、ここには来ない。**読めないことを「チャンクがない」と扱わない。**

### ファイルの扱い

種別ごとのファイルについて、次の順に判定する。

| 条件 | 扱い |
|---|---|
| サイズが 0 | 出力しない（理由: `EmptyFile`。clean の規則） |
| 三つ組がそのままコピー | そのままコピー |
| 削除するエントリがない | そのままコピー |
| 残すエントリがない | 出力しない（理由: `NoEntriesLeft`） |
| それ以外 | 詰め直す（残すインデックスの一覧） |

### 外部ファイル（`.mcc`）

| 条件 | 扱い |
|---|---|
| 削除するエントリが参照している | 出力しない |
| それ以外（三つ組がそのままコピー、残すエントリが参照している、どのエントリも参照していない） | コピーする |

### リージョンファイル以外のファイル

| 対象 | 扱い |
|---|---|
| プレイヤーデータの一時ファイル（`^[0-9a-f-]{36}-\d+\.dat$`、プレイヤーデータの場所にあるもの） | 出力しない（理由: `PlayerTempFile`。clean の規則） |
| それ以外のすべてのファイル | そのままコピー |

Plan には、出力しないファイルの一覧だけを持つ。そのままコピーするファイルの列挙は P7 が行う。

### 報告のみ（削除しない）

| 対象 | 内容 |
|---|---|
| 古い形式のチャンク | チャンクの DataVersion が `level.dat` の DataVersion より小さいものの数。公式の最適化を勧める根拠 |
| 本体のない `<uuid>.dat_old` | 本体の `<uuid>.dat` がないものを、異常として報告する |

## 見積もり

ディメンションごと、種別ごとに:

| 項目 | 計算 |
|---|---|
| 現在のサイズ | リージョンファイルと `.mcc` のサイズの合計 |
| 出力後のサイズ | そのままコピー: 現在のサイズ。詰め直し: `8192 + Σ ceil((4 + L) / 4096) × 4096`。出力しない: 0。これに、コピーする `.mcc` のサイズを加える |

全体の出力サイズ（P7 の空き容量の確認に使う）は、全ディメンションの出力後のサイズと、そのままコピーする他のファイルのサイズの合計。

## 古い地形との境界

削除するチャンクと上下左右で隣接する、残すチャンクのうち、`has_blending_data` が真のものを集める。これらは、削除したチャンクが再生成されたときに、境目で段差が出る。

- 8 近傍で連結しているものを1つのまとまりにし、まとまりごとに「チャンク数」と「範囲（最小と最大のチャンク座標）」を出す。
- ディメンションごとに、範囲の昇順で並べる。

## Plan のモデル

P6 と P7 はこのモデルだけを使う。

```rust
pub struct Plan {
    pub world_data_version: Option<i32>,
    pub gc_dims: Option<Vec<String>>,              // None なら全ディメンション
    pub dimensions: Vec<DimensionPlan>,            // ID の昇順
    pub omitted_other_files: Vec<OmittedFile>,     // パスの昇順
    pub protection_unknown: Vec<ProtectionUnknown>,// 強制読み込みチャンクを確認できなかったディメンション
    pub reports: Reports,                          // 報告のみの項目
    pub estimate: Estimate,
}

pub struct DimensionPlan {
    pub id: String,
    pub dir: PathBuf,
    pub world_folder: PathBuf,
    pub triples: Vec<TriplePlan>,                  // 座標の昇順
    pub boundary_warnings: Vec<BoundaryCluster>,
}

pub struct TriplePlan {
    pub rx: i32,
    pub rz: i32,
    pub verbatim_reason: Option<Vec<StructuralIssue>>, // Some ならそのままコピー
    pub region: Option<FilePlan>,
    pub entities: Option<FilePlan>,
    pub poi: Option<FilePlan>,
    pub chunks: Vec<ChunkDecision>,                // region のチャンク。インデックスの昇順
}

pub struct FilePlan {
    pub path: PathBuf,
    pub size: u64,
    pub action: FileAction,                        // Verbatim | Rewrite { keep: Vec<u16> } | Omit(OmitReason)
    pub removed: Vec<(u16, RemoveReason)>,         // GcDeleted | Orphan
    pub external: Vec<ExternalPlan>,               // .mcc のパスと、Copy か Omit か
}

pub struct ChunkDecision {
    pub index: u16,
    pub pos: (i32, i32),
    pub inhabited_time: Option<i64>,               // 内容エラーなら None
    pub data_version: Option<i32>,
    pub has_blending_data: bool,
    pub content_error: Option<ContentError>,
    pub decision: Decision,                        // Delete | Keep(KeepReason)
}
```

メモリについて: 巨大なワールドでは、チャンクの数が数百万になる。`structure_refs` は段階 B の保護の計算にだけ使い、Plan には残さない。

## 受け入れ条件

- InhabitedTime が 0 のチャンクだけが削除され、タグなし、型の違い、内容エラーのチャンクは残る。
- 強制読み込みチャンクの外側 2 チャンクまでが保護され、3 チャンク目は保護されない。
- InhabitedTime が 0 より大きいチャンクから参照されている開始点のチャンクが保護される。リージョン三つ組をまたぐ参照でも同じ。InhabitedTime が 0 のチャンクからの参照では保護されない。
- どれかのファイルに構造エラーがある三つ組では、何も削除されない。
- region にチャンクがない entities／poi のエントリが `Orphan` で削除され、region が読めない場合は削除されない。
- サイズ 0 のファイルとプレイヤーデータの一時ファイルが、出力しないファイルになる。
- `--dim` で対象外のディメンションでは、region のチャンクが削除されず、clean の規則だけが適用される。
- 同じ入力から2回作った Plan が一致する。
