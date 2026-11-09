# wgc 仕様（MVP）

Minecraft Java Edition のワールド（Anvil 形式）から不要なチャンクを削除する CLI。詳細は `docs/spec/` にある。

## 読み方

各フェーズは、この文書と、下の表の文書・節だけを読めば実装できる。

| フェーズ | 内容 | 読む文書 | 他の文書で読む節 |
|---|---|---|---|
| P0 | ワークスペースの構成 | この文書のみ | — |
| P1 | リージョンファイルの読み書き | [region-format.md](spec/region-format.md) | — |
| P2 | チャンク NBT の要約 | [chunk-nbt.md](spec/chunk-nbt.md)、[versions.md](spec/versions.md) | region-format「圧縮方式」 |
| P3 | ワールドの構成の認識 | [world-layout.md](spec/world-layout.md)、[versions.md](spec/versions.md) | region-format「ファイル名」 |
| P4 | 計画 | [plan.md](spec/plan.md) | region-format「エラーの区分」、chunk-nbt「ChunkSummary」、world-layout「ディメンションマップ」「リージョン三つ組の列挙」「強制読み込みチャンク」「プレイヤーデータ」 |
| P5 | InhabitedTime の信用の確認 | [trust.md](spec/trust.md)、[versions.md](spec/versions.md) | world-layout「level.dat」「ゲームルール」「プレイヤーデータ」「Paper の設定ファイル」、plan「Plan のモデル」 |
| P6 | 診断（scan） | [scan.md](spec/scan.md) | plan「Plan のモデル」、trust「結果のモデル」、cli「固定の注意文」 |
| P7 | 出力と検証 | [output.md](spec/output.md) | plan「Plan のモデル」、region-format「書き込み」、world-layout「ディメンションマップ」、trust「判定の使い方」、cli「停止と `--force`」 |
| P8 | CLI | [cli.md](spec/cli.md) | scan「ターミナル出力」、output「`wgc verify`」「書き戻しの手順」、trust「判定の使い方」 |

## 基本方針

- 元のワールドには書き込まない。結果は新しいフォルダに書き出し、ユーザーが手動で入れ替える。
- ゴミ箱は持たない。元のワールドが復元の手段になる。
- 削除するのは InhabitedTime が 0 のチャンクだけ。判断できないものは残す。
- チャンクの中身は再エンコードしない。残すチャンクは元のバイト列のまま書く。
- 判定の結果は1つの Plan にまとめ、scan、見積もり、実行で共有する。
- 出力は元のワールドと突き合わせて検証し、通った場合だけ完了とする。
- ツールが出すメッセージは英語のみ。

## コマンド

```
wgc scan   <path> [--json <file>]    # 診断（読み取りのみ）
wgc gc     <path> [--out <dir>] ...  # --out がなければ見積もりのみ
wgc verify <path> <out>              # 書き戻してよいかの確認
```

`<path>` は、バニラではワールドフォルダ、Bukkit 系ではサーバーのルートフォルダ。

## 用語

| 用語 | 意味 |
|---|---|
| ワールドフォルダ | `level.dat` があるフォルダ |
| ディメンションフォルダ | `region/`、`entities/`、`poi/` を直接持つフォルダ |
| 種別 | `region`、`entities`、`poi` のどれか |
| リージョン三つ組 | 同じディメンション・同じリージョン座標の3種別のファイル。処理の単位 |
| チャンクの位置 | リージョン座標とファイル内のインデックスから求めた絶対チャンク座標 |
| そのままコピー | 内容を変えずに出力にコピーすること |
| clean の規則 | サイズ 0 のリージョンファイル、プレイヤーデータの一時ファイル、本体のない entities／poi のデータを出力しない規則。`gc` で常に適用する |

## 技術構成（P0）

| 項目 | 方針 |
|---|---|
| 言語 | Rust（edition 2024） |
| 構成 | ワークスペース。`crates/core`（`wgc-core`）と `crates/cli`（`wgc`、バイナリ） |
| core のモジュール | `region`、`nbt`、`layout`、`plan`、`trust`、`report`、`output` |
| 依存 | `cli` → `core`。`core` は CLI の依存を持たず、端末に出力しない |
| 並列化 | リージョン三つ組の単位 |
| エラー | `core` は型つきのエラーを返し、`cli` が整形する |

P0 の完了条件: `cargo build`、`cargo test`、`wgc --help` が動く。

## テスト

- 各文書の「受け入れ条件」を自動テストで満たす。
- 合成したリージョンファイルを基本にする。実際のワールドの切り出しは `tests/fixtures/` に置き、作成したバージョンを書く。
- チャンクの形式は、1.18 より前（`Level` あり）、1.18 以降、最新の3種類を用意する。

## 根拠

保存形式とバージョンの値は、2026-11-09 時点の次の資料で確認した。

- Minecraft 26.3 のサーバー jar（難読化なし）を逆コンパイルしたもの: `DataFixers`、各 `FileFix`、`TicketStorage`、`GameRuleRegistryFix`、`DirectoryLock`、`SerializableChunkData`、`RegionFileVersion`
- 1.16 のスナップショットの公式マッピング（`DirectoryLock` の導入時期）
- PaperMC/Paper（26.3、commit `b31b4b5`）と、Issue #12827
- lz4-java の `LZ4BlockOutputStream`

## MVP に入れないもの

- 対象外: DataFixerUpper の再実装、プレイヤーの痕跡による判定、`.dat_old` の削除、ゴミ箱、ユーザーが指定する保護範囲、サーバーの種類の履歴による判定、スポーンチャンクの保護。
- 後回し: 自動の入れ替え（`wgc swap`）、元のワールドを直接書き換えるモード、clean の規則だけの実行、旧形式（`.mcr` など）の掃除、HTML レポート、単体のデフラグ、しきい値による削除、公式の最適化の自動実行、reflink によるコピー。
