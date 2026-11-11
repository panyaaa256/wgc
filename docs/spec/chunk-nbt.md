# チャンク NBT の要約

入力は、region のチャンクを展開した NBT（名前つきのルートの Compound）。値を使うのは、ここに挙げるフィールドだけ。

## 読むフィールド

| フィールド | 型 | DataVersion ≥ 2844 | それより前 |
|---|---|---|---|
| DataVersion | Int | ルート | ルート（ないことがある） |
| InhabitedTime | Long | ルート | `Level` の下 |
| blending_data | Compound | ルート（有無だけ） | — |
| 構造物の参照 | Compound（値は LongArray） | `structures.References` | `Level.Structures.References` |

2844 は versions.md の `DV_CHUNK_ROOT`。

- ルートと `Level` の両方を見る。DataVersion が 2844 以上ならルートの値、2844 未満かないなら `Level` の値を使う。選んだ側にタグがなければ「タグなし」。
- 型が違うタグは「タグなし」として扱う。エラーにも報告にもしない。
- `Status`、`xPos`、`zPos` は読まない。
- `blending_data` は DataVersion ≥ 2844 のときだけ数える。
- 同じキーが重複していたら、ゲームと同じく最後のものを使う。
- キーの名前は、文字コードを検証せずにバイト列で比べる。
- ルートの Compound の後ろに余分なバイトがあっても、ゲームと同じく無視する。

### 構造物の参照

`References` の各 LongArray の各値は、チャンクの位置を Long に詰めたもの（types.md「座標」）。構造物の ID は問わず、1つの集合にまとめる。

## 要約する値

1つのチャンクから、次の値を求める。

| 値 | 内容 |
|---|---|
| DataVersion | チャンクの DataVersion。ないこともある |
| InhabitedTime | 値、または「タグなし」（タグがない、または型が違う）。負の値もそのまま扱う（扱いは plan.md） |
| blending_data | あるかどうか |
| 構造物の参照 | 参照されているチャンクの位置の集合 |

## 区別する失敗

NBT として読めなければ、そのチャンクは内容エラー C-2 になる（region-format.md「エラーの区分」）。次の失敗を区別する。

| 失敗 | 意味 |
|---|---|
| 途中で終わる | データが NBT の途中で終わる |
| 不明なタグの型 | タグの型の値が NBT にないもの |
| 文字列が不正 | 文字列として読めない |
| ルートが Compound でない | ルートのタグが Compound ではない |
| その他の不正 | 上記以外（長さの不正、深すぎる入れ子など） |

## entities／poi

MVP では展開しない。構造エラーの検査だけを行う。

## 受け入れ条件

- `Level` あり（DataVersion < 2844、および DataVersion なし）とルート直下の両形式で、全フィールドを読める。
- InhabitedTime がない場合と型が違う場合に「タグなし」になる。
- 負の座標を含む参照（例: `x = -1, z = 2`）を正しく復元できる。
- 壊れた NBT は C-2 になり、異常終了しない（ファズテストを1つ用意する）。
