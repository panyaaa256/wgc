# 計画（P4）

> 前提:
> - [region-format.md](region-format.md) の「エラーの区分」（構造エラーと内容エラー）
> - [chunk-nbt.md](chunk-nbt.md) の「ChunkSummary」
> - [world-layout.md](world-layout.md) の「ディメンションマップ」「リージョン三つ組の列挙」「強制読み込みチャンク」「スポーン」「プレイヤーデータ」
>
> 実装先: `wgc-core` の `plan` モジュール。

## 目的

ワールドを読み、「どのチャンクを消し、どのファイルをどう出力するか」を決めた **Plan** を作る。Plan はただのデータで、ファイルには書き込まない。

- `scan`、`gc`／`clean` の見積もり、`gc`／`clean` の実行（P7）は、すべて同じ Plan を使う。
- 実行のときは、Plan を作り直す。過去の `scan` の出力を使い回さない。
- 同じ入力からは、同じ Plan ができる（並び順も含めて決定的にする）。

## モード

| モード | 適用する規則 |
|---|---|
| `Gc { dims }` | GC の規則 + clean の規則。`dims` が指定されていれば、GC の削除はそのディメンションだけで行う。clean の規則は全ディメンションに適用する |
| `Clean` | clean の規則だけ |

## 処理の流れ

### 段階 A: リージョン三つ組ごと（並列）

各リージョン三つ組について:

1. `region`、`entities`、`poi` の各ファイルを解析し、構造エラーを集める。
2. 3つのうちどれかに構造エラーがあれば、三つ組を **そのままコピー** とする（理由: どのファイルに、どの構造エラーがあるか）。
   - region に構造エラーがあると、entities／poi の対応が信用できない。entities／poi に構造エラーがあると、region のチャンクを消したときに、対応するエンティティや POI を消せない。どちらの場合も、三つ組全体を触らない。
3. region の各チャンクを展開して `ChunkSummary` を作る。失敗したチャンクは内容エラーとして記録する。
   - 三つ組がそのままコピーの場合も、読めるチャンクは要約する（構造物の参照を集めるため）。読めないものは無視して、報告だけにする。

### 段階 B: 全体（逐次）

1. **削除候補** C: GC の対象のディメンションの、そのままコピーでない三つ組にある、`inhabited_time == Some(0)` のチャンク。
2. **保護の集合** P を作る。
   - 強制読み込み: 各強制読み込みチャンクと、そこからチェビシェフ距離 2 以内のチャンク（同じディメンション）。
   - スポーン: スポーン地点のチャンクから、チェビシェフ距離「エンティティが処理される半径 + 2」以内のチャンク（スポーンのディメンション）。
3. **構造物の参照の不動点**:
   ```
   loop {
       K = すべての region のチャンク − (C − P)      // 残すチャンク
       R = K のチャンクの structure_refs の和集合（同じディメンション）
       P' = P ∪ (R ∩ C)
       if P' == P { break }
       P = P'
   }
   ```
   保護したチャンクの参照も、さらに保護の対象になる。参照はリージョン三つ組をまたぐので、この段階は全体で行う。
4. 削除するチャンク D = C − P。

強制読み込みチャンクかスポーンが「確認できない」場合、その保護は計算できない。Plan にその旨を記録する（`protection_unknown`）。`gc` の実行は、[trust.md](trust.md) の「確認できない場合」に従って中止する。`scan` と見積もりでは、その保護がないものとして計算し、その旨を表示する。

## 判定の規則

### region のチャンク

削除するのは、次のすべてを満たすチャンクだけ。

- モードが `Gc` で、そのディメンションが GC の対象である
- 三つ組がそのままコピーでない
- 内容エラーがない
- `inhabited_time == Some(0)`
- 保護の集合 P に含まれない

それ以外は残す。残す理由は、次の優先順位で1つ記録する。

| 優先 | 理由 | 意味 |
|---|---|---|
| 1 | `TripleVerbatim` | 三つ組がそのままコピー |
| 2 | `NotTargeted` | `Clean` モード、または GC の対象外のディメンション |
| 3 | `ContentError` | 展開できない、NBT を読めない |
| 4 | `InhabitedUnknown` | InhabitedTime のタグがない、または型が違う |
| 5 | `Inhabited` | InhabitedTime が 0 より大きい |
| 6 | `Forceload` | 強制読み込みによる保護 |
| 7 | `Spawn` | スポーンによる保護 |
| 8 | `StructureRef` | 構造物の参照による保護 |

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
| 三つ組がそのままコピー | コピーする |
| 残すエントリが参照している | コピーする |
| 削除するエントリが参照している | 出力しない |
| どのエントリも参照していない | コピーする（「参照されていない外部ファイル」として報告する） |

### リージョンファイル以外のファイル

| 対象 | 扱い |
|---|---|
| プレイヤーデータの一時ファイル（`^[0-9a-f-]{36}-\d+\.dat$`、プレイヤーデータの場所にあるもの） | 出力しない（理由: `PlayerTempFile`。clean の規則） |
| それ以外のすべてのファイル | そのままコピー |

Plan には、出力しないファイルの一覧だけを持つ。そのままコピーするファイルの列挙は P7 が行う。

### 報告のみ（削除しない）

| 対象 | 内容 |
|---|---|
| `<uuid>.dat_old` | 本体の `<uuid>.dat` がないものを、異常として報告する |
| 古い形式のチャンク | チャンクの DataVersion が `level.dat` の DataVersion より小さいものの数。公式の最適化を勧める根拠 |
| 位置の不一致 | `nbt_pos` がファイル内の位置と一致しないチャンク |
| 型の違い | `type_mismatches` があるチャンク |
| 参照されていない外部ファイル | 上記 |
| 不明なディメンション | [world-layout.md](world-layout.md) の W-3 |

## 見積もり

ディメンションごと、種別ごとに:

| 項目 | 計算 |
|---|---|
| 現在のサイズ | リージョンファイルと `.mcc` のサイズの合計 |
| 出力後のサイズ | そのままコピー: 現在のサイズ。詰め直し: `8192 + Σ ceil((4 + L) / 4096) × 4096`。出力しない: 0。これに、コピーする `.mcc` のサイズを加える |
| 断片化 | 各ファイルについて、`ファイルサイズ − (8192 + 使用中のセクタ数 × 4096)` |

全体の出力サイズ（P7 の空き容量の確認に使う）は、全ディメンションの出力後のサイズと、そのままコピーする他のファイルのサイズの合計。

## 古い地形との境界

削除するチャンクと上下左右で隣接する、残すチャンクのうち、`has_blending_data` が真のものを集める。これらは、削除したチャンクが再生成されたときに、境目で段差が出る。

- 8 近傍で連結しているものを1つのまとまりにし、まとまりごとに「チャンク数」と「範囲（最小と最大のチャンク座標）」を出す。
- ディメンションごとに、範囲の昇順で並べる。

## Plan のモデル

P6 と P7 はこのモデルだけを使う。

```rust
pub struct Plan {
    pub mode: Mode,
    pub world_data_version: Option<i32>,
    pub dimensions: Vec<DimensionPlan>,            // ID の昇順
    pub omitted_other_files: Vec<OmittedFile>,     // パスの昇順
    pub protection_unknown: Vec<ProtectionUnknown>,// 強制読み込み・スポーンを確認できなかったもの
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
    pub summary: Option<ChunkSummary>,             // 内容エラーなら None
    pub content_error: Option<ContentError>,
    pub decision: Decision,                        // Delete | Keep(KeepReason)
}
```

メモリについて: 巨大なワールドでは、チャンクの数が数百万になる。`ChunkSummary` のうち、Plan の作成後に必要なもの（InhabitedTime、DataVersion、blending_data の有無）だけを残し、`structure_refs` などは段階 B の後に捨ててよい。

## 受け入れ条件

- InhabitedTime が 0 のチャンクだけが削除され、タグなし、型の違い、内容エラーのチャンクは残る。
- 強制読み込みチャンクの外側 2 チャンクまでが保護され、3 チャンク目は保護されない。
- スポーンの保護の範囲が、半径 + 2 になる。
- 構造物の参照の連鎖（A が B を参照し、保護された B が C を参照する）で、B と C が両方保護される。リージョン三つ組をまたぐ参照でも同じ。
- どれかのファイルに構造エラーがある三つ組では、何も削除されない。
- region にチャンクがない entities／poi のエントリが `Orphan` で削除され、region が読めない場合は削除されない。
- サイズ 0 のファイルとプレイヤーデータの一時ファイルが、出力しないファイルになる。
- `Clean` モードでは、region のチャンクが削除されない。
- 同じ入力から2回作った Plan が一致する。
