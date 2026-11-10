# 出力と検証（P7）

実装先: `core::output`。

元のワールドは読み取り専用で開く。例外は `session.lock` のロック（lock.md）だけ。

```rust
pub fn run_gc(
    layout: &WorldLayout,
    options: &GcOptions,         // out, gc_dims, allowed: BTreeSet<GuardId>（--force で続行を許す ID）
    observer: &dyn Observer,
    cancel: &AtomicBool,
) -> Result<GcOutcome, OutputError>;   // GcOutcome: Plan、TrustReport、forced、ScanReport の材料

pub fn verify(out: &Path, source: Option<&Path>) -> Result<VerifyOutcome, VerifyError>;
```

## 実行の順序（`gc --out`）

| 順 | 段階 | 失敗したとき |
|---|---|---|
| 1 | 出力先の確認 | 中止（何も書かない） |
| 2 | `SessionLocks::try_acquire_all` | 中止。`Missing` は L-1（段階 3 の結果とまとめて判定する） |
| 3 | Plan を作る前の信用の確認（T-1〜T-5。`check_trust_before_plan`。`--force` で T-3 が許されていれば T-3 は段階 6 だけで確かめる） | L-1 と合わせて中止（`--force` で許された ID は続行） |
| 4 | 元のワールドの状態を記録（メモリ上） | 中止 |
| 5 | Plan を作る | 中止 |
| 6 | Plan に対する信用の確認（`check_trust`） | 中止（`--force` で許された ID は続行） |
| 7 | 空き容量の確認 | 中止 |
| 8 | 出力を書く | `failure.json` を書いて終了 |
| 9 | 検証 | 同上 |
| 10 | `source-state.json`、`complete.json` を書く | 同上 |
| 11 | ロックを解放 | — |

段階 1〜7 で中止したときは、出力先のフォルダを作らない。`--force` で許されていない停止は、すべて段階 3 でわかる（すべてのリージョンファイルを読み終えるまで待たせない）。段階 6 の結果は、その間にファイルが変わらない限り段階 3 と同じになる（L-1 を無視してロックなしで続行した場合への備え）。`--force` で許された停止の警告（`Warning::Forced`、`Warning::LockSkipped`）は、その段階で続行が決まったときだけ出す。

中断（`cancel`）されたら、その場で止め、ロックを解放して `OutputError::Cancelled` を返す。出力先は `complete.json` も `failure.json` もないまま残る（不完全な出力）。

## 出力先

- 存在しないパスか、空のフォルダに限る。元のワールドの中、または元のワールドを含むパスは不可（存在する部分を正規化して比べる）。まだ存在しない部分に `..` を含むパスは不可（`InvalidOutputPath`）。
- バニラ: 出力先がワールドフォルダになる。Bukkit: 出力先の直下に各ワールドフォルダを同じ名前で置く（サーバーのルートの他のファイルは出力しない）。
- 出力先の直下に、記録用の `.wgc/` を作る。

## 空き容量

`estimate.total_output_bytes × 1.01 + 64 MiB` が、出力先（なければ最も近い親）の空き容量を超えたら中止する。

## 書き出し

各ワールドフォルダの下のすべてのファイル:

| ファイル | 扱い |
|---|---|
| Plan にあるリージョンファイル | `TriplePlan::action` に従う |
| Plan にある `.mcc` | `TriplePlan::external_action` に従う |
| `omitted_other_files` | 出力しない |
| `session.lock`（ワールドフォルダの直下） | 出力しない（ゲームが起動時に作る） |
| ワールドフォルダの直下の `.wgc/` | 出力しない（以前の実行の記録） |
| それ以外 | そのままコピー |

- 空のディレクトリも作る。
- シンボリックリンクは辿り、実体を書く。同じディレクトリに複数のリンクがあれば、それぞれのパスに書く。自分の祖先を指すリンク（ループ）はエラー。辿ったら `Warning::SymlinkFollowed`。リンク先がないリンクは `Warning::SpecialFileSkipped`。
- 通常のファイルとディレクトリ以外は出力せず、`Warning::SpecialFileSkipped`。
- ハードリンクは使わない（Minecraft はリージョンファイルをその場で上書きするので、元のワールドも書き換わる）。
- 各ファイルは、内容を書き、更新日時、所有者（root のとき）、権限の順に合わせてから fsync する。最後に作ったディレクトリを fsync する（Unix）。
- 権限のビットと更新日時は元のファイルに合わせる。root で実行したら所有者とグループも合わせる（所有者を持たないファイルシステムでは、失敗にせず警告する）。root でなく、所有者が実行ユーザーと違うときは `Warning::OwnerNotPreserved` を一度だけ出す。Windows は読み取り専用の属性だけ。

## 検証

| ID | 検証 |
|---|---|
| V-1 | 詰め直したファイルに構造エラーがない |
| V-2 | 詰め直したファイルのエントリの集合が、Plan の残すエントリと一致する |
| V-3 | 詰め直したファイルの残すエントリのペイロードとタイムスタンプが、元と一致する |
| V-4 | そのままコピーしたファイル（`.mcc` を含む）のサイズが、元と一致する |
| V-5 | 出力先のファイルの集合が、Plan から独立に求めた「出力すべきファイル」の集合と一致する（`.wgc/` を除く） |
| V-6 | 元のワールドの状態が、段階 4 の記録と一致する |

失敗したら、すべての失敗を `failure.json` に書く。

## 記録（`<out>/.wgc/`）

パスの書き方は types.md「レポートと JSON」に従う。

### `source-state.json`

各ワールドフォルダの下の全ファイル（`session.lock` を含む）の、相対パス、サイズ、更新日時（ナノ秒）。`stat` で得る。

```json
{
  "schema": "wgc.source-state/1",
  "source": "/abs/path/to/input",
  "layout": "bukkit_legacy",
  "world_folders": ["world", "world_nether", "world_the_end"],
  "files": [{ "path": "world/level.dat", "size": 1234, "mtime_ns": 0 }]
}
```

### `complete.json`

すべてを書いて fsync し、検証が通った後に、一時ファイルに書いてから rename で置き、ディレクトリを fsync する。これがない出力、または `failure.json` がある出力は不完全。

```json
{
  "schema": "wgc.complete/1",
  "tool_version": "0.1.0",
  "finished_at": "2026-11-09T12:34:56Z",
  "source": "/abs/path/to/input",
  "summary": { "deleted_chunks": 0, "size_before": 0, "size_after": 0 },
  "forced": ["T-3"]
}
```

### `failure.json`

```json
{
  "schema": "wgc.failure/1",
  "tool_version": "0.1.0",
  "stage": "verify",
  "error": "…",
  "items": [{ "check": "V-3", "path": "…", "detail": "…" }]
}
```

- `stage`: `write`、`verify`、`record`。
- 失敗した出力は消さずに残す（不具合の報告に添付してもらう）。

## `wgc verify`

読み取りのみ。`source` を省略したら `source-state.json` の `source` を使う。

1. `complete.json` がない、または `failure.json` があれば `VerifyError::Incomplete`。
2. `source` を指定していて、記録と（両方を正規化して）違えば `SourceMismatch`。
3. 各ワールドフォルダの下を `stat` し、記録と比べる。追加・削除・サイズか更新日時の変更があれば `Changed`（変わったファイルの一覧）。
4. `SessionLock::is_locked` が真なら `InUse`。
5. すべて通れば `VerifyOutcome { forced }` を返す。

## エラー

```rust
pub enum OutputError {
    OutputNotEmpty { path: PathBuf },
    InvalidOutputPath { path: PathBuf },
    OutputOverlapsSource { out: PathBuf, world: PathBuf },
    Lock(LockError),
    Stopped(Vec<GuardId>),                 // 信用の確認か L-1 で止まった
    InsufficientSpace { needed: u64, available: u64 },
    Layout(LayoutError),
    Plan(PlanError),
    Verification(Vec<VerifyFailure>),      // V-1..V-6
    Io { path: PathBuf, source: io::Error },
    Cancelled,
}

pub enum VerifyError {
    Incomplete { out: PathBuf },
    SourceMismatch { recorded: PathBuf, given: PathBuf },
    Changed(Vec<ChangedFile>),             // Added | Removed | Modified
    InUse { path: PathBuf },
    Io { path: PathBuf, source: io::Error },
}
```

## 書き戻しの手順

`gc --out` の完了時と `wgc verify` の成功時に、CLI が実際のパスで表示する。

1. ゲーム（サーバー）を止める。
2. `wgc verify <out>` が成功することを確かめる。
3. `<path>` を `<path>.wgc-bak` に名前を変える。
4. `<out>` を `<path>` に移す。
5. 起動して確かめる。
6. 問題がなければ `<path>.wgc-bak` を削除する。削除すると元に戻せないので、しばらく残すことを勧める。
7. `<path>/.wgc/` は削除してよい。

Bukkit では、3 と 4 を各ワールドフォルダについて行い、`.wgc/` は移さない。シンボリックリンクを実体にした場合と、所有者を合わせられなかった場合は、その旨も表示する。

## 受け入れ条件

- 出力先が空でない、元のワールドの中、元のワールドを含む場合に、何も書かずに中止する。
- GC した出力で V-1〜V-6 が通り、`complete.json` ができる。
- 書き出しの途中で失敗させると、`complete.json` がなく `failure.json` がある。
- `cancel` を立てると `Cancelled` で戻り、ロックが解放され、`complete.json` がない。
- 出力の権限と更新日時が元と一致する。
- `wgc verify` が、`level.dat` を書き換えた場合と、`session.lock` がロックされている場合に失敗する。
