# 共通の型と規約

実装先: `wgc-core` の `types`（基本の型）と各モジュール。全フェーズで、ここの型と規約に従う。

`wgc-core` は公開しない内部ライブラリなので、フィールドは `pub` でよく、`#[non_exhaustive]` や sealed トレイトは使わない。

## 基本の型

生の `i32`、`u16`、タプル、`String` で座標やバージョンを表さない。

```rust
pub struct RegionPos { pub x: i32, pub z: i32 }

pub struct ChunkPos { pub x: i32, pub z: i32 }
impl ChunkPos {
    pub fn from_packed(v: i64) -> Self;   // x = 下位 32 ビット、z = 上位 32 ビット
    pub fn region(self) -> RegionPos;     // (x >> 5, z >> 5)
    pub fn index(self) -> ChunkIndex;     // (x & 31) + (z & 31) * 32
}

pub struct ChunkIndex(u16);               // 0..1024。範囲外は作れない
impl ChunkIndex {
    pub fn new(i: u16) -> Option<Self>;
    pub fn to_chunk_pos(self, region: RegionPos) -> ChunkPos;
}

pub struct DataVersion(pub i32);          // Ord。定数は versions.md

pub struct DimensionId(String);           // "<ns>:<name>" の形に検証済み
impl DimensionId {
    pub fn parse(s: &str) -> Result<Self, DimensionIdError>; // "overworld" などは "minecraft:" を補う
    pub fn overworld() -> Self; pub fn the_nether() -> Self; pub fn the_end() -> Self;
}

pub enum FileKind { Region, Entities, Poi }

pub enum Known<T> { Value(T), Unknown(UnknownReason) } // 読めたか、確認できなかったか

pub enum GuardId { T1, T2, T3, T4, T5, L1 }  // 表示と JSON は "T-1" ... "L-1"
```

- チャンク座標を Long から戻す処理は `ChunkPos::from_packed` だけで行う（構造物の参照、`Forced`、`chunk_pos`）。
- `UnknownReason`（trust.md の「結果のモデル」に列挙）も `types` に置く。layout が「確認できない」を返すときに使うため。

## 派生と決定性

- モデルの型は `#[derive(Debug, Clone, PartialEq, Eq)]` を付ける（`f64` を持つ型は `Eq` を除く）。座標などの小さい型は `Copy`、`Ord`、`Hash` も付ける。
- すべての型は `Send + Sync` にする（並列処理のため）。
- モデルの中では `HashMap`／`HashSet` を使わない。集合と対応は `BTreeSet`／`BTreeMap`、または並べた `Vec`。
- 並列処理の結果は、位置の順に並べてから集める。

## 警告・進捗・中断

`wgc-core` は端末に出力しない。長い処理の関数は、次の2つを引数に取る。

```rust
pub trait Observer: Send + Sync {
    fn progress(&self, p: Progress);
    fn warn(&self, w: Warning);
}

pub enum Progress {
    Reading { done: u64, total: u64 },   // リージョン三つ組の数
    Writing { done: u64, total: u64 },
    Verifying { done: u64, total: u64 },
}

pub enum Warning {
    LockUnavailable(LockError),           // scan でロックを取れなかった
    LockSkipped,                          // L-1 を --force で無視した
    PreLockVersion(DataVersion),          // DV_SESSION_LOCK_OS より古い
    OwnerNotPreserved,                    // 一度だけ
    SymlinkFollowed(PathBuf),
    SpecialFileSkipped(PathBuf),
    Forced(GuardId),                      // --force で無視した確認
}
```

- 中断は `&AtomicBool` で受け取り、リージョン三つ組ごとに確かめる。立っていたら、各モジュールのエラーの `Cancelled` で戻る。
- 文章は `wgc-core` で作らない。英語のメッセージは CLI の1か所にまとめる（cli.md）。

## エラー

- モジュールごとに thiserror でエラーの enum を定義する（`NbtError`、`LayoutError`、`LockError`、`PlanError`、`OutputError`、`VerifyError`）。各文書の「エラー」の節が定義する。
- どれも `std::error::Error + Send + Sync + 'static`。
- I/O のエラーはパスを付けて包む: `Io { path: PathBuf, #[source] source: io::Error }`。
- `Display` は小文字で始め、末尾に句点を付けない（ログ用）。利用者向けの文は CLI が作る。
- CLI は、エラーの種類から終了コードを決める（cli.md「終了コード」）。

## レポートと JSON

- JSON に出す型は `core::report` に、Plan などとは別に定義し、`serde::Serialize` と `#[serde(rename_all = "snake_case")]` を付ける。
- パスは入力のパスからの相対パスで、区切りは `/`。入力のパスそのものは `"."`。UTF-8 でないパスがあれば、レポートの作成をエラーにする。例外として `source` は正規化した絶対パス。
- `.wgc/` の JSON も同じ規則に従う。

## 受け入れ条件

- `ChunkPos::from_packed` が負の座標を正しく戻す。`ChunkIndex::new(1024)` が `None` になる。`ChunkPos::index` と `to_chunk_pos` が往復する。
- `DimensionId::parse` が省略形を補い、形の不正な ID を拒否する。
- `GuardId` が `"T-1"` 形式で表示・シリアライズされる。
