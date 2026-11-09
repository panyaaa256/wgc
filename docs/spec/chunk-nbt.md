# チャンク NBT の要約（P2）

> 前提: [region-format.md](region-format.md) の「圧縮方式」（展開した NBT のバイト列を受け取る）、[versions.md](versions.md)。
> 実装先: `wgc-core` の `nbt` モジュール。

## 目的

展開したチャンクの NBT から、判定に必要なフィールドだけを読み、`ChunkSummary` を作る。それ以外のフィールドは読み飛ばす。チャンクを書き戻すことはない（ペイロードは元のバイト列のままコピーする）。

## NBT ライブラリの扱い

- nanonbt を使う。依存は git の特定のコミットに固定する（`rev = "..."`）。
  - git の依存があると crates.io に公開できない。MVP では公開しない前提とする。
- **nanonbt を使うのは `core::nbt` モジュールの中だけ。** 他のモジュールには、次の関数と、依存のない普通の構造体だけを公開する。
  - `read_chunk_summary(nbt: &[u8]) -> Result<ChunkSummary, NbtError>`
  - `level.dat` とプレイヤーデータを読む関数（必要なフィールドは [world-layout.md](world-layout.md) と [trust.md](trust.md) に書いてある。P3 と P5 でこのモジュールに追加する）
- これで、NBT ライブラリを差し替えるときの変更が、このモジュールに閉じる。
- nanonbt の derive を使う場合、必要なフィールドだけを持つ構造体を定義し、それ以外は読み飛ばす。タグがないことを区別できるようにする（`Option`、または番兵値）。

## 入力

- 展開した、Java 版のファイル用の NBT（名前つきのルートの Compound）。
- NBT として読めない場合は `NbtError` を返す。呼び出し側は、これを内容エラー C-2（[region-format.md](region-format.md)）として扱う。

## 読むフィールド

| フィールド | 型 | DataVersion ≥ 2844（`DV_CHUNK_ROOT`） | それより前 |
|---|---|---|---|
| DataVersion | Int | ルート直下 | ルート直下（1.9 より前はない） |
| InhabitedTime | Long | ルート直下 | `Level` の下 |
| blending_data | Compound | ルート直下（あるかどうかだけ） | — |
| 構造物の参照 | Compound（値は LongArray） | `structures.References` | `Level.Structures.References` |

### 形式の選び方

- 1つの構造体で読む。ルート直下のフィールドと、`Level`（任意）の下のフィールドを、両方とも任意として定義する。2回パースしない。
- どちらの値を使うか:
  - ルートの DataVersion が 2844 以上なら、ルート直下の値を使う。
  - DataVersion が 2844 未満、またはない場合は、`Level` の下の値を使う。
- 選んだ側にタグがなければ、「タグなし」とする。もう一方の側を代わりに使うことはしない。

### 型が違う場合

- タグはあるが、型が想定と違う場合（例: InhabitedTime が Int）は、**「タグなし」と同じに扱う**。InhabitedTime の場合は「判断できない」になり、チャンクは残る。
- 型の違いはエラーにせず、報告もしない。

### 構造物の参照

- `References` は、構造物の ID をキー、LongArray を値とする Compound。
- 各 Long は、構造物の開始点を持つチャンクの位置を表す。
  - `x = (value & 0xFFFFFFFF) as i32`（下位 32 ビット）
  - `z = (value >> 32) as i32`（上位 32 ビット）
- 構造物の ID は問わず、すべての位置を1つの集合にまとめる。
- `starts` は読まない（P4 は、参照されている位置を構造物の種類に関係なく保護する）。

## ChunkSummary

```rust
pub struct ChunkSummary {
    pub data_version: Option<i32>,
    pub inhabited_time: Option<i64>,   // None = タグなし、または型の違い
    pub has_blending_data: bool,
    pub structure_refs: Vec<(i32, i32)>, // 重複を除き、昇順に並べる
}
```

- `Status`、`xPos`、`zPos` は読まない。
  - `Status` は判定に使わない。バージョンの変換で書き換わることがあるため（1.13〜1.14 の変換で完成済みのチャンクに `spawn` が付く、1.18 の変換で `structure_starts` などに戻される）。
  - チャンクの識別は、NBT の `xPos`／`zPos` ではなく、ファイル内の位置で行う（リージョンファイルをリネームしてコピーしたワールドでは、両者が一致しないことがある）。

## entities／poi

MVP では、`entities/` と `poi/` のチャンクは展開しない。構造の検査（[region-format.md](region-format.md) の構造エラー）だけを行い、位置で `region/` と対応づける。

## 性能

- InhabitedTime の NBT の中での位置は決まっていない（HashMap の順序）ので、途中で読むのを打ち切らない。チャンク全体を展開してから読む。
- 展開用のバッファは、スレッドごとに使い回す。

## 受け入れ条件

- `Level` を持つ形式（DataVersion < 2844、および DataVersion なし）と、ルート直下の形式の両方で、全フィールドを読める。
- InhabitedTime がない場合と、型が違う場合に、`inhabited_time` が `None` になる。
- `References` の Long から、負の座標を含む位置を正しく復元できる（例: `x = -1, z = 2`）。
- 読めない NBT で `NbtError` を返し、パニックしない（ファズテストを1つ用意する）。
- 読み飛ばすフィールドに大きなデータ（セクションなど）がある実際のチャンクで、正しく要約できる。
