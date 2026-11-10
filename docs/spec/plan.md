# 計画（P4）

実装先: `core::plan`。ワールドを読み、何を消してどう出力するかを決めた Plan を作る。ファイルには書き込まない。

- `scan`、見積もり、`gc --out` は同じ Plan を使う。`gc --out` では作り直す。
- 同じ入力からは、並び順も含めて同じ Plan ができる。
- GC の規則と clean の規則を常に両方適用する。`--dimension` があれば、GC の削除はそのディメンションだけで行う（clean の規則は全ディメンション）。

```rust
pub fn build_plan(
    layout: &WorldLayout,
    options: &PlanOptions,       // gc_dims: Option<BTreeSet<DimensionId>>
    observer: &dyn Observer,
    cancel: &AtomicBool,
) -> Result<Plan, PlanError>;
```

## 処理の流れ

### 段階 A: リージョン三つ組ごと（並列）

1. 3ファイルを解析し、構造エラーを集める。
2. どれかに構造エラーがあれば、三つ組を **そのままコピー** にする。
3. region の各チャンクを展開して `ChunkSummary` を作る。失敗は内容エラーとして記録する。そのままコピーの三つ組でも、読めるチャンクは要約する（構造物の参照を集めるため）。

### 段階 B: 全体

1. 削除候補 C: GC の対象のディメンションの、そのままコピーでない三つ組にある、`Inhabited::Value(0)` のチャンク。
2. 強制読み込みによる保護 F: 各強制読み込みチャンクと、チェビシェフ距離 2 以内のチャンク。
3. 構造物の参照による保護 R: 削除候補でないチャンク（正の値、`Missing`、`Negative`、`Unreadable`、そのままコピーの三つ組、GC の対象外）と、F で残すチャンクの `structure_refs` の和集合。参照は三つ組をまたぐ。1回だけ見る（R で残したチャンクの参照はたどらない）。
4. 削除するチャンク D = C − F − R。

強制読み込みチャンクが「確認できない」ディメンションは `protection_unknown` に記録する。`gc --out` では T-5、`scan` と見積もりでは F なしで計算する。

## 判定

### InhabitedTime

```rust
pub enum Inhabited {
    Value(i64),              // 0 以上
    Missing,                 // タグなし、型の違い
    Negative(i64),           // 負の値。判断できないものとして扱う
    Unreadable(ContentError),
}
```

`ChunkSummary.inhabited_time` が `None` なら `Missing`、負なら `Negative`、それ以外は `Value`。展開か NBT の読み込みに失敗したら `Unreadable`。

### region のチャンク

D に入るものだけ削除する。残すものには、次の優先順位で理由を1つ記録する。

| 優先 | `KeepReason` |
|---|---|
| 1 | `TripleVerbatim`（そのままコピーの三つ組） |
| 2 | `NotTargeted`（`--dimension` の対象外） |
| 3 | `ContentError`（`Unreadable`） |
| 4 | `InhabitedUnknown`（`Missing`、`Negative`） |
| 5 | `Inhabited`（正の値） |
| 6 | `Forceload` |
| 7 | `StructureRef` |

### entities／poi のエントリ

そのままコピーの三つ組ではすべて残す。それ以外は、同じ位置の region のチャンクで決める。

| region の同じ位置 | `EntryFate` |
|---|---|
| 残す | `Keep` |
| 削除する | `Remove(GcDeleted)` |
| チャンクがない（ファイルがない、サイズ 0、エントリが空） | `Remove(Orphan)` |

region が読めない場合は三つ組がそのままコピーになっている。読めないことを「チャンクがない」と扱わない。

### ファイル

| 条件（上から順に） | `FileAction` |
|---|---|
| サイズ 0 | `Omit(EmptyFile)` |
| そのままコピーの三つ組 | `Verbatim` |
| 削除するエントリがない | `Verbatim` |
| 残すエントリがない | `Omit(NoEntriesLeft)` |
| それ以外 | `Rewrite { keep }` |

`.mcc` は、削除するエントリが参照しているものだけ出力せず、それ以外はコピーする。リージョンファイルのない `.mcc` は三つ組に属さないので、他のファイルと同じくそのままコピーする。

### リージョンファイル以外

プレイヤーデータの場所にある一時ファイル（`^[0-9a-f-]{36}-\d+\.dat$`）は出力しない（`OmitReason::PlayerTempFile`）。それ以外はすべてそのままコピー（列挙は P7）。

### 報告のみ

- 古い形式のチャンクの数（チャンクの DataVersion < `level.dat` の DataVersion）。`ChunkDecision` から数える。
- 本体の `<uuid>.dat` がない `<uuid>.dat_old`。

## 見積もり

ディメンション・種別ごとに、現在のサイズ（`.mcc` を含む）と出力後のサイズを出す。出力後のサイズは、`Verbatim` なら現在のサイズ、`Rewrite` なら `8192 + Σ ceil((4 + L) / 4096) × 4096`、`Omit` なら 0 に、コピーする `.mcc` を加えたもの。

`total_output_bytes`（P7 の空き容量の確認用）には、そのままコピーする他のファイルも含める。

## 古い地形との境界

D と上下左右で隣接する、残すチャンクのうち `has_blending_data` のものを集め、8 近傍で連結したまとまりごとに、チャンク数と範囲を出す。

## Plan のモデル

残すかどうかの情報源は `ChunkDecision.decision` だけにする。ファイルとエントリの扱いはメソッドで導く。

```rust
pub struct Plan {
    pub layout: WorldLayout,
    pub gc_dims: Option<BTreeSet<DimensionId>>,     // None = 全ディメンション
    pub dimensions: Vec<DimensionPlan>,             // ID の昇順
    pub omitted_other_files: Vec<OmittedFile>,      // パスの昇順
    pub protection_unknown: BTreeSet<DimensionId>,
    pub dat_old_without_dat: Vec<PathBuf>,
    pub estimate: Estimate,
}

pub struct DimensionPlan {
    pub dimension: Dimension,
    pub triples: Vec<TriplePlan>,                   // 座標の昇順
    pub boundary_warnings: Vec<BoundaryCluster>,
}

pub struct TriplePlan {
    pub pos: RegionPos,
    pub issues: Vec<StructuralIssue>,               // 空でなければそのままコピー
    pub region: Option<FileInfo>,
    pub entities: Option<FileInfo>,
    pub poi: Option<FileInfo>,
    pub chunks: Vec<ChunkDecision>,                 // region のチャンク。インデックスの昇順
}

impl TriplePlan {
    pub fn is_verbatim(&self) -> bool;
    pub fn action(&self, kind: FileKind) -> Option<FileAction>;      // ファイルがなければ None
    pub fn entry_fate(&self, kind: FileKind, index: ChunkIndex) -> EntryFate;
    pub fn external_action(&self, kind: FileKind, pos: ChunkPos) -> ExternalAction; // Copy | Omit
    pub fn sizes(&self, kind: FileKind) -> Sizes;   // そのファイルと .mcc の現在と出力後のサイズ
}

pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub entries: Vec<EntryInfo>,                    // インデックスの昇順
    pub externals: Vec<ExternalFile>,               // 参照される .mcc
}

pub struct EntryInfo { pub index: ChunkIndex, pub length: u32, pub external: bool }
pub struct ExternalFile { pub pos: ChunkPos, pub path: PathBuf, pub size: u64 }

pub struct ChunkDecision {
    pub index: ChunkIndex,
    pub pos: ChunkPos,
    pub inhabited: Inhabited,
    pub data_version: Option<DataVersion>,
    pub has_blending_data: bool,
    pub decision: Decision,
}

pub enum Decision { Delete, Keep(KeepReason) }
pub enum FileAction { Verbatim, Rewrite { keep: Vec<ChunkIndex> }, Omit(OmitReason) }
pub enum OmitReason { EmptyFile, NoEntriesLeft, PlayerTempFile }
pub enum EntryFate { Keep, Remove(RemoveReason) }
pub enum RemoveReason { GcDeleted, Orphan }

pub struct OmittedFile { pub path: PathBuf, pub reason: OmitReason }
pub struct BoundaryCluster { pub chunks: u32, pub min: ChunkPos, pub max: ChunkPos }

pub struct Estimate {
    pub dimensions: BTreeMap<DimensionId, KindSizes>,
    pub other_files_bytes: u64,
    pub total_output_bytes: u64,
}
pub struct KindSizes { pub region: Sizes, pub entities: Sizes, pub poi: Sizes }
pub struct Sizes { pub before: u64, pub after: u64 }
```

`structure_refs` は段階 B でだけ使い、Plan には残さない。

## エラー

```rust
pub enum PlanError {
    Io { path: PathBuf, source: io::Error },   // ファイルを開けない・読めない（権限など）
    Layout(LayoutError),                       // 列挙の失敗（同じ座標の2つの名前など）
    Cancelled,
}
```

壊れたファイルや壊れたチャンクはエラーにせず、Plan の中で扱う（そのままコピー、内容エラー）。OS がファイルを読ませない場合（権限など）は、判断の材料がないので中止する。

## 受け入れ条件

- InhabitedTime が 0 のチャンクだけが削除され、タグなし、型の違い、負の値、内容エラーは残る。
- 強制読み込みチャンクの外側 2 チャンクまで保護され、3 チャンク目は保護されない。
- InhabitedTime が 0 でないチャンクから参照された開始点は、三つ組をまたいでも保護される。0 のチャンクからの参照では保護されない。
- 構造エラーのある三つ組では何も削除されない。
- region にチャンクがないエントリは `Orphan` で削除され、region が読めない場合は削除されない。
- サイズ 0 のファイルとプレイヤーデータの一時ファイルが出力されない。
- `--dimension` の対象外では、clean の規則だけが適用される。
- 同じ入力から作った2つの Plan が `==` で等しい。
- `cancel` を立てると `PlanError::Cancelled` で戻る。
