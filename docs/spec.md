# wgc 仕様（MVP）

Minecraft Java Edition のワールド（Anvil 形式）から不要なチャンクを削除する CLI。

この仕様は、wgc が**何をするか**（振る舞い、安全性の保証、外部とのやり取り）を定める。内部の設計（型、API、アルゴリズム、並列化）は定めない。

## 文書の一覧

| 文書 | 内容 |
|---|---|
| [types.md](spec/types.md) | 共通の規約（座標、決定性、警告、進捗、中断） |
| [json.md](json.md) | JSON の出力の形（`--json` と `<out>/.wgc/`） |
| [region-format.md](spec/region-format.md) | リージョンファイルの形式、構造エラーと内容エラー、書き込みの形式 |
| [chunk-nbt.md](spec/chunk-nbt.md) | チャンク NBT から読む値 |
| [versions.md](spec/versions.md) | バージョンの参照表 |
| [world-layout.md](spec/world-layout.md) | ワールドの構成の認識と、読むファイル |
| [lock.md](spec/lock.md) | `session.lock` のロック |
| [plan.md](spec/plan.md) | 削除と保持の判定、見積もり |
| [trust.md](spec/trust.md) | InhabitedTime を信用してよいかの確認 |
| [scan.md](spec/scan.md) | 診断（`wgc scan`）の内容 |
| [output.md](spec/output.md) | 出力、検証、記録、`wgc verify` |
| [cli.md](spec/cli.md) | コマンド、オプション、終了コード、表示 |

## 基本方針

- 元のワールドには書き込まない。結果は新しいフォルダに書き出し、ユーザーが手動で入れ替える。
- ゴミ箱は持たない。元のワールドが復元の手段になる。
- 削除するのは InhabitedTime が 0 のチャンクだけ。判断できないものは残す。
- チャンクの中身は再エンコードしない。残すチャンクは元のバイト列のまま書く。
- scan、見積もり、実行は、同じ入力に対して同じ判定の結果を示す。
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
| リージョン三つ組 | 同じディメンション・同じリージョン座標の3種別のファイル |
| チャンクの位置 | リージョン座標とファイル内のインデックスから求めた絶対チャンク座標 |
| そのままコピー | 内容を変えずに出力にコピーすること |
| clean の規則 | サイズ 0 のリージョンファイル、プレイヤーデータの一時ファイル、本体のない entities／poi のデータを出力しない規則。`gc` で常に適用する |

## 技術構成

### ワークスペース

- ルートの `Cargo.toml` は仮想マニフェストにする。`resolver = "3"`。
- `[workspace.package]`: `edition = "2024"`、`license = "MIT"`（nanonbt は MIT OR Apache-2.0）。
- ツールチェーンはルートの `rust-toolchain.toml` で `1.99.1` に固定する（components: rustfmt、clippy、rust-analyzer）。`rust-version`（MSRV）は宣言しない。
- `[workspace.dependencies]` で依存の版をまとめる。nanonbt は `git` と `rev` で固定する。
- `crates/core`（`wgc-core`、ライブラリ）と `crates/cli`（`wgc`、バイナリ）。どちらも `publish = false`。
- 配布: GitHub Releases のバイナリと、`cargo install --git <repo> --locked`（git の依存があるので crates.io には公開できない）。

### 境界

- 依存の向きは `cli` → `core`。`core` は CLI の依存を持たず、端末に出力しない（警告、進捗、中断は types.md の規約で `cli` に渡す）。
- OS の呼び出し（unsafe）は `core` の1つのモジュールの中だけに置く。
- NBT ライブラリは `core` の1つのモジュールの中だけで使う。

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
- 後回し: 自動の入れ替え（`wgc swap`）、元のワールドを直接書き換えるモード、clean の規則だけの実行、旧形式（`.mcr` など）の掃除、HTML レポート、単体のデフラグ、しきい値による削除、公式の最適化の自動実行、reflink によるコピー、`wgc-core` の公開。
