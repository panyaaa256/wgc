# session.lock のロック（P3）

実装先: `core::lock`。unsafe は `core::lock::sys` の中だけに置く。

## 要件

- Java の `FileChannel.tryLock` と互いに排他になる OS のロックを、待たずに試す。
- ゲームは起動時に `session.lock` を作成し、`☃` を書いて fsync してからロックする。そのため、ロック中にゲームを起動しようとしても起動しない。また、ゲームが起動すると更新日時が必ず変わる（`wgc verify` はこれを使う）。
- 20w14a（`DV_SESSION_LOCK_OS`）より前のゲームはロックを使わないので、起動中かどうかを検出できない。

## 実装

| OS | 方法 | crate |
|---|---|---|
| Linux | OFD ロック（`fcntl(F_OFD_SETLK)`、`F_WRLCK`、ファイル全体） | `libc` |
| その他の Unix | `fcntl(F_SETLK)` | `libc` |
| Windows | `LockFileEx`（`LOCKFILE_EXCLUSIVE_LOCK \| LOCKFILE_FAIL_IMMEDIATELY`） | `windows-sys` |

- Unix で `flock` は使わない（Linux では Java の `fcntl` ロックと干渉しない。`File::try_lock`、`fs2`、`fs4` も `flock`）。
- POSIX ロックは、同じプロセスで同じファイルの別の記述子を閉じると解放される。ロック中は `session.lock` を他の目的で開かない（`stat` はよい）。
- 読み書き・作成なし・切り詰めなしで開き、中身は書き換えない。

## API

```rust
pub struct SessionLock { /* 開いたファイル */ }   // Drop で解放する

impl SessionLock {
    pub fn try_acquire(world_folder: &Path) -> Result<SessionLock, LockError>;
    pub fn is_locked(world_folder: &Path) -> Result<bool, LockError>; // 取れたらすぐ解放
}

pub struct SessionLocks(Vec<SessionLock>);
impl SessionLocks {
    pub fn try_acquire_all(layout: &WorldLayout) -> Result<SessionLocks, LockError>; // 正規化して重複を除き、順にロック
}
```

## エラー

```rust
pub enum LockError {
    InUse { path: PathBuf },
    Missing { path: PathBuf },        // session.lock がない（L-1）
    NotWritable { path: PathBuf },    // 書き込みで開けない
    HeldByThisProcess { path: PathBuf }, // このプロセスがすでに持っている（開き直すとロックが外れうるので拒否する）
    Io { path: PathBuf, source: io::Error },
}
```

## コマンドごとの扱い

| コマンド | `InUse` | `Missing` | `NotWritable`、`Io` |
|---|---|---|---|
| `gc --out` | 停止 | L-1 で停止（`--force` でロックなしで続行し、`Warning::LockSkipped`） | 停止（サーバーと同じユーザーで実行するよう案内する） |
| `scan`、`gc` の見積もり | `Warning::LockUnavailable` で続行 | 同左 | 同左 |
| `verify` | 失敗 | 失敗しない（ロックを使わない状態） | 失敗 |

ロックを取れたワールドの DataVersion が `DV_SESSION_LOCK_OS` より小さければ、`Warning::PreLockVersion` を出す。

## 受け入れ条件

- Java が `session.lock` をロックしている間は `InUse` になり、`SessionLock` を持っている間は Java がロックを取れない（Linux の統合テスト）。
- `SessionLock` を持ったまま他の処理（ワールドの読み込み、出力）をしても、ロックが保持されている。
- `Drop` で解放される。
