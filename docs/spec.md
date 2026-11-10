# wgc 仕様（MVP）

Minecraft Java Edition のワールド（Anvil 形式）から不要なチャンクを削除する CLI。詳細は `docs/spec/` にある。

## 読み方

各フェーズは、この文書、[types.md](spec/types.md)（P1 以降すべて）、下の表の文書・節だけを読めば実装できる。

| フェーズ | 内容 | 読む文書 | 他の文書で読む節 |
|---|---|---|---|
| P0 | ワークスペースの構成 | この文書のみ | — |
| P1 | リージョンファイルの読み書き | [region-format.md](spec/region-format.md) | — |
| P2 | チャンク NBT の要約 | [chunk-nbt.md](spec/chunk-nbt.md)、[versions.md](spec/versions.md) | region-format「圧縮方式」 |
| P3 | ワールドの構成の認識とロック | [world-layout.md](spec/world-layout.md)、[lock.md](spec/lock.md)、[versions.md](spec/versions.md) | region-format「ファイル名」 |
| P4 | 計画 | [plan.md](spec/plan.md)、[versions.md](spec/versions.md) | region-format「エラーの区分」、chunk-nbt「ChunkSummary」、world-layout「WorldLayout」「リージョン三つ組の列挙」「level.dat」「強制読み込みチャンク」「プレイヤーデータ」 |
| P5 | InhabitedTime の信用の確認 | [trust.md](spec/trust.md)、[versions.md](spec/versions.md) | world-layout「WorldLayout」「level.dat」「ゲームルール」「プレイヤーデータ」「Paper の設定ファイル」、plan「Plan のモデル」 |
| P6 | 診断（scan） | [scan.md](spec/scan.md) | plan「Plan のモデル」「見積もり」、trust「結果のモデル」、world-layout「WorldLayout」、lock「API」「コマンドごとの扱い」、cli「固定の注意文」 |
| P7 | 出力と検証 | [output.md](spec/output.md)、[versions.md](spec/versions.md) | plan「Plan のモデル」「見積もり」、region-format「書き込み」、world-layout「WorldLayout」、lock「API」「コマンドごとの扱い」、trust「判定の使い方」 |
| P8 | CLI | [cli.md](spec/cli.md) | scan「JSON」「ターミナル出力」、output「`wgc verify`」「エラー」「書き戻しの手順」、trust「結果のモデル」「判定の使い方」、world-layout「エラー」、lock「エラー」、plan「エラー」 |

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
wgc scan   <path> [--json]           # 診断（読み取りのみ）
wgc gc     <path> [-o <dir>] ...     # -o がなければ見積もりのみ
wgc verify <out>                     # 書き戻してよいかの確認
```

`<path>` は、バニラではワールドフォルダ、Bukkit 系ではサーバーのルートフォルダ。詳細は cli.md。

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

### ワークスペース

- ルートの `Cargo.toml` は仮想マニフェストにする。`resolver = "3"`。
- `[workspace.package]`: `edition = "2024"`、`license = "MIT"`（nanonbt は MIT OR Apache-2.0）。
- ツールチェーンはルートの `rust-toolchain.toml` で `1.99.1` に固定する（components: rustfmt、clippy、rust-analyzer）。`rust-version`（MSRV）は宣言しない。
- `[workspace.dependencies]` で依存の版をまとめる。nanonbt は `git` と `rev` で固定する。
- `crates/core`（`wgc-core`、ライブラリ）と `crates/cli`（`wgc`、バイナリ）。どちらも `publish = false`。
- 配布: GitHub Releases のバイナリと、`cargo install --git <repo> --locked`（git の依存があるので crates.io には公開できない）。

### モジュールと依存

| 項目 | 方針 |
|---|---|
| core のモジュール | `types`、`region`、`nbt`、`layout`、`lock`、`plan`、`trust`、`report`、`output` |
| 依存の向き | `cli` → `core`。`core` は CLI の依存を持たず、端末に出力しない（types.md「警告・進捗・中断」） |
| 並列化 | リージョン三つ組の単位（`rayon`） |

| crate | 用途 | 使う側 |
|---|---|---|
| nanonbt（git 固定） | NBT | core（`nbt` の中だけ） |
| flate2（zlib-rs バックエンド） | gzip、zlib | core |
| lz4_flex | LZ4 のブロックの展開 | core |
| xxhash-rust（`xxh32`） | LZ4 のチェックサム | core |
| yaml-rust2 | Paper の設定 | core |
| libc / windows-sys | ロック | core（`sys` の中だけ） |
| thiserror | エラー | core |
| serde、serde_json | JSON | core（`report`、`output`） |
| rayon | 並列化 | core |
| clap | 引数 | cli |
| indicatif | 進捗の表示 | cli |

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

設計は Rust API Guidelines と Command Line Interface Guidelines（clig.dev）でレビューした。

## MVP に入れないもの

- 対象外: DataFixerUpper の再実装、プレイヤーの痕跡による判定、`.dat_old` の削除、ゴミ箱、ユーザーが指定する保護範囲、サーバーの種類の履歴による判定、スポーンチャンクの保護。
- 後回し: 自動の入れ替え（`wgc swap`）、元のワールドを直接書き換えるモード、clean の規則だけの実行、旧形式（`.mcr` など）の掃除、HTML レポート、単体のデフラグ、しきい値による削除、公式の最適化の自動実行、reflink によるコピー、`wgc-core` の公開（公開するなら `#[non_exhaustive]` などを見直す）。
