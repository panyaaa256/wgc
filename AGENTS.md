# Project Context

This workspace contains wgc, a chunk GC for Minecraft Java Edition worlds written in Rust. It deletes chunks that no player has ever been near (`InhabitedTime = 0`) and writes the result to a new folder, never modifying the original world.

# Reference Documentation

Prior to generating, refactoring, or reviewing code, strictly follow the specifications in:
- **`docs/spec.md`**: Overall specification, terminology, workspace layout, module boundaries, and dependency policy.
- **`docs/spec/*.md`**: Detailed specifications per topic (CLI, world layout, region format, chunk NBT, locking, trust checks, scan, output, and versions).

# Code Quality & Refactoring Directives

- **Concise & Idiomatic Rust**: Write clean, modern, and idiomatic Rust. Prefer simplified expressions, standard combinators (e.g., `Option`/`Result` combinators), and clean control flow over overly verbose logic.
- **Signal-to-Noise Ratio in Comments**:
  - **Avoid Redundant Comments**: Do not write comments that merely restate what the code clearly expresses (e.g., `// create a new vector`, `// return the result`).
  - **Keep Value-Additive Comments**: Retain or add comments only when explaining complex algorithm choices, subtle invariants, safety guarantees (`// SAFETY:`), or architectural context.
- **Self-Documenting Code**: Prefer expressive function, variable, and type names over heavy comment explanations.

# Code Generation Directives

- Respect crate boundaries inside `crates/*` and adhere strictly to the unidirectional dependency flow defined in `docs/spec.md` (`cli` -> `core`).
- Never use `unwrap()`, `expect()`, or `panic!` in non-test library code.
- Write code comments, rustdoc, and commit messages in English. The specifications under `docs/` are written in Japanese.

# Commit Directives

- Write commit messages in the [Conventional Commits](https://www.conventionalcommits.org/) format: `<type>[optional scope]: <description>`.
  - **Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
  - **Scope**: the affected area, such as a crate or module (`core`, `cli`, `region`, `output`, `spec`).
  - **Description**: imperative mood, present tense, under 72 characters.
  - Mark a breaking change with `!` after the type or scope, or with a `BREAKING CHANGE:` footer.
- Keep one logical change per commit.
