# チャンク NBT の要約（P2）

実装先: `core::nbt`。入力は展開した NBT（名前つきルートの Compound）。

## NBT ライブラリ

- nanonbt を git の特定コミットに固定して使う（MVP では crates.io に公開しない）。
- nanonbt を使うのは `core::nbt` の中だけ。外には次の関数と、依存のない構造体だけを公開する。
  - `read_chunk_summary(nbt: &[u8]) -> Result<ChunkSummary, NbtError>`
  - `level.dat` とプレイヤーデータを読む関数（P3、P5 で追加）
- nanonbt の derive は `Option` と「型が違えば無視」に対応しないので、低レベルの API（`SliceReader`、`read_name_into`、`skip_nbt`）で必要なフィールドだけを読み、それ以外は読み飛ばす。

## 読むフィールド

| フィールド | 型 | DataVersion ≥ 2844 | それより前 |
|---|---|---|---|
| DataVersion | Int | ルート | ルート（ないことがある） |
| InhabitedTime | Long | ルート | `Level` の下 |
| blending_data | Compound | ルート（有無だけ） | — |
| 構造物の参照 | Compound（値は LongArray） | `structures.References` | `Level.Structures.References` |

- 1つの構造体で、ルートと `Level` の両方のフィールドを任意として読む（2回パースしない）。
- DataVersion が 2844 以上ならルートの値、2844 未満かないなら `Level` の値を使う。選んだ側にタグがなければ「タグなし」。
- 型が違うタグは「タグなし」として扱う。エラーにも報告にもしない。
- `Status`、`xPos`、`zPos` は読まない。

### 構造物の参照

`References` の各 LongArray の各値を `ChunkPos::from_packed` でチャンクの位置にする。構造物の ID は問わず、1つの集合にまとめる。

## ChunkSummary

```rust
pub struct ChunkSummary {
    pub data_version: Option<DataVersion>,
    pub inhabited_time: Option<i64>,       // None = タグなし、または型の違い。負の値もそのまま返す
    pub has_blending_data: bool,
    pub structure_refs: BTreeSet<ChunkPos>,
}

pub enum NbtError {
    Truncated,
    InvalidTagType(u8),
    InvalidString,
    RootNotCompound,
    Malformed,          // その他（長さの不正、深すぎる入れ子など）
}
```

NBT として読めなければ `NbtError` を返す（呼び出し側で内容エラー C-2）。

## entities／poi

MVP では展開しない。構造エラーの検査だけを行う。

## 受け入れ条件

- `Level` あり（DataVersion < 2844、および DataVersion なし）とルート直下の両形式で、全フィールドを読める。
- InhabitedTime がない場合と型が違う場合に `None` になる。
- 負の座標を含む参照（例: `x = -1, z = 2`）を正しく復元できる。
- 壊れた NBT で `NbtError` を返し、パニックしない（ファズテストを1つ用意する）。
