# 計画（P4）

実装先: `core::plan`。ワールドを読み、何を消してどう出力するかを決めた Plan を作る。ファイルには書き込まない。

- `scan`、見積もり、`gc --out` は同じ Plan を使う。`gc --out` では作り直す。
- 同じ入力からは、並び順も含めて同じ Plan ができる。
- GC の規則と clean の規則を常に両方適用する。`--dim` があれば、GC の削除はそのディメンションだけで行う（clean の規則は全ディメンション）。

## 処理の流れ

### 段階 A: リージョン三つ組ごと（並列）

1. 3ファイルを解析し、構造エラーを集める。
2. どれかに構造エラーがあれば、三つ組を **そのままコピー** にする。
3. region の各チャンクを展開して `ChunkSummary` を作る。失敗は内容エラーとして記録する。そのままコピーの三つ組でも、読めるチャンクは要約する（構造物の参照を集めるため）。

### 段階 B: 全体

1. 削除候補 C: GC の対象のディメンションの、そのままコピーでない三つ組にある、`inhabited_time == Some(0)` のチャンク。
2. 強制読み込みによる保護 F: 各強制読み込みチャンクと、チェビシェフ距離 2 以内のチャンク。
3. 構造物の参照による保護 R: InhabitedTime が 0 でないチャンク（0 より大きい、タグなし、内容エラー、そのままコピーの三つ組）の `structure_refs` の和集合。参照は三つ組をまたぐ。1回だけ見る。
4. 削除するチャンク D = C − F − R。

強制読み込みチャンクが「確認できない」ディメンションは `protection_unknown` に記録する。`gc --out` では T-5、`scan` と見積もりでは F なしで計算して表示する。

## 判定

### region のチャンク

D に入るものだけ削除する。残すものには、次の優先順位で理由を1つ記録する。

| 優先 | 理由 |
|---|---|
| 1 | `TripleVerbatim`（そのままコピーの三つ組） |
| 2 | `NotTargeted`（`--dim` の対象外） |
| 3 | `ContentError` |
| 4 | `InhabitedUnknown`（タグなし、型の違い） |
| 5 | `Inhabited`（0 より大きい） |
| 6 | `Forceload` |
| 7 | `StructureRef` |

### entities／poi のエントリ

そのままコピーの三つ組ではすべて残す。それ以外は、同じ位置の region のチャンクで決める。

| region の同じ位置 | 判定 |
|---|---|
| 残す | 残す |
| 削除する | 削除（`GcDeleted`） |
| チャンクがない（ファイルがない、サイズ 0、エントリが空） | 削除（`Orphan`） |

region が読めない場合は三つ組がそのままコピーになっている。読めないことを「チャンクがない」と扱わない。

### ファイル

| 条件（上から順に） | 扱い |
|---|---|
| サイズ 0 | 出力しない（`EmptyFile`） |
| そのままコピーの三つ組 | そのままコピー |
| 削除するエントリがない | そのままコピー |
| 残すエントリがない | 出力しない（`NoEntriesLeft`） |
| それ以外 | 詰め直す |

`.mcc` は、削除するエントリが参照しているものだけ出力せず、それ以外はコピーする。

### リージョンファイル以外

プレイヤーデータの場所にある一時ファイル（`^[0-9a-f-]{36}-\d+\.dat$`）は出力しない（`PlayerTempFile`）。それ以外はすべてそのままコピー（列挙は P7）。

### 報告のみ

- 古い形式のチャンクの数（チャンクの DataVersion < `level.dat` の DataVersion）。
- 本体の `<uuid>.dat` がない `<uuid>.dat_old`。

## 見積もり

ディメンション・種別ごとに、現在のサイズ（`.mcc` を含む）と出力後のサイズを出す。出力後のサイズは、そのままコピーなら現在のサイズ、詰め直しなら `8192 + Σ ceil((4 + L) / 4096) × 4096`、出力しないなら 0 に、コピーする `.mcc` を加えたもの。

全体の出力サイズ（P7 の空き容量の確認用）には、そのままコピーする他のファイルも含める。

## 古い地形との境界

D と上下左右で隣接する、残すチャンクのうち `has_blending_data` のものを集め、8 近傍で連結したまとまりごとに、チャンク数と範囲（最小・最大の座標）を出す。

## Plan のモデル

```rust
pub struct Plan {
    pub world_data_version: Option<i32>,
    pub gc_dims: Option<Vec<String>>,          // None = 全ディメンション
    pub dimensions: Vec<DimensionPlan>,        // ID の昇順
    pub omitted_other_files: Vec<OmittedFile>, // パスの昇順
    pub protection_unknown: Vec<String>,       // ディメンションの ID
    pub reports: Reports,
    pub estimate: Estimate,
}

pub struct DimensionPlan {
    pub id: String,
    pub dir: PathBuf,
    pub world_folder: PathBuf,
    pub triples: Vec<TriplePlan>,              // 座標の昇順
    pub boundary_warnings: Vec<BoundaryCluster>,
}

pub struct TriplePlan {
    pub rx: i32,
    pub rz: i32,
    pub verbatim_reason: Option<Vec<StructuralIssue>>,
    pub region: Option<FilePlan>,
    pub entities: Option<FilePlan>,
    pub poi: Option<FilePlan>,
    pub chunks: Vec<ChunkDecision>,            // インデックスの昇順
}

pub struct FilePlan {
    pub path: PathBuf,
    pub size: u64,
    pub action: FileAction,                    // Verbatim | Rewrite { keep: Vec<u16> } | Omit(OmitReason)
    pub removed: Vec<(u16, RemoveReason)>,     // GcDeleted | Orphan
    pub external: Vec<ExternalPlan>,           // .mcc と Copy / Omit
}

pub struct ChunkDecision {
    pub index: u16,
    pub pos: (i32, i32),
    pub inhabited_time: Option<i64>,
    pub data_version: Option<i32>,
    pub has_blending_data: bool,
    pub content_error: Option<ContentError>,
    pub decision: Decision,                    // Delete | Keep(KeepReason)
}
```

`structure_refs` は段階 B でだけ使い、Plan には残さない。

## 受け入れ条件

- InhabitedTime が 0 のチャンクだけが削除され、タグなし、型の違い、内容エラーは残る。
- 強制読み込みチャンクの外側 2 チャンクまで保護され、3 チャンク目は保護されない。
- InhabitedTime が 0 より大きいチャンクから参照された開始点は、三つ組をまたいでも保護される。0 のチャンクからの参照では保護されない。
- 構造エラーのある三つ組では何も削除されない。
- region にチャンクがないエントリは `Orphan` で削除され、region が読めない場合は削除されない。
- サイズ 0 のファイルとプレイヤーデータの一時ファイルが出力されない。
- `--dim` の対象外では、clean の規則だけが適用される。
- 同じ入力から同じ Plan ができる。
