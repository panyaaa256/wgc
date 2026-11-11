# Contributing

Start with [`ARCHITECTURE.md`](ARCHITECTURE.md) for the layout of the code and
the invariants it keeps. Behavior is defined by
[`docs/gc-rules.md`](docs/gc-rules.md), [`docs/world-format.md`](docs/world-format.md),
[`docs/json.md`](docs/json.md) and the [README](README.md).

## Workspace

- A virtual manifest with two crates: `crates/core` (`wgc-core`, library) and
  `crates/cli` (`wgc`, binary). Neither is published.
- The toolchain is pinned in `rust-toolchain.toml` (with rustfmt, clippy and
  rust-analyzer). No MSRV is declared.
- Dependency versions are declared once in `[workspace.dependencies]`.
  `nanonbt` is pinned by git revision, which is why wgc cannot be published on
  crates.io ([ADR-0010](docs/adr/0010-parse-nbt-with-nanonbt.md)). Releases
  ship as GitHub Releases binaries and via `cargo install --git <repo> --locked`.
- Dependency licenses and sources are checked by `cargo deny` (`deny.toml`).

## Checks

CI runs the following; run them locally before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace        # or: cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
cargo machete
typos
```

## Code

- `cli` depends on `core`, never the reverse. `core` never prints; see
  [`ARCHITECTURE.md`](ARCHITECTURE.md#invariants).
- No `unwrap()`, `expect()` or `panic!` in non-test code.
- Comments explain what the code cannot: invariants, non-obvious choices,
  `// SAFETY:` notes. Do not restate the code.
- Code, comments and tests must not refer to the documents under `docs/`; state
  the rule itself. User-facing IDs (T-1, S-3, V-5, ...) are defined in the
  rustdoc of the type that carries them
  ([ADR-0011](docs/adr/0011-keep-docs-behavioral-and-code-self-explanatory.md)).

## Tests

- Every behavior in the reference documents should be covered by a test.
- Build region files synthetically. Excerpts of real worlds go under
  `tests/fixtures/` together with the game version that produced them.
- Cover three chunk formats: before 1.18 (with `Level`), 1.18 and later, and
  the latest version.
- JSON shapes are pinned by byte-identical golden tests. Changing a shape means
  updating the test and [`docs/json.md`](docs/json.md) together.

## Documentation

- All documentation is written in English. `README_ja.md` is a Japanese
  translation of the README and must be kept in sync with it.
- Documents describe behavior, external facts and contracts, not the
  implementation.
- Record significant decisions as ADRs in [`docs/adr/`](docs/adr/), using
  [`0000-template.md`](docs/adr/0000-template.md). Do not rewrite an accepted
  ADR; supersede it with a new one.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/):
`<type>[optional scope]: <description>`.

- Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`,
  `ci`, `chore`, `revert`.
- Scope: the affected area, such as `core`, `cli`, `region`, `output`.
- Description: imperative mood, present tense, under 72 characters.
- Mark breaking changes with `!` or a `BREAKING CHANGE:` footer.
- One logical change per commit.
