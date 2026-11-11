# Project Context

This workspace contains wgc, a chunk GC for Minecraft Java Edition worlds written in Rust. It deletes chunks that no player has ever been near (`InhabitedTime = 0`) and writes the result to a new folder, never modifying the original world.

# Reference Documentation

The specifications define **what** wgc must do, not **how** it is built:
- **`docs/spec.md`**: Overall principles, terminology, workspace layout, module boundaries, and dependency policy.
- **`docs/spec/*.md`**: Behavior per topic (CLI, world layout, region format, chunk NBT, locking, trust checks, scan, output, and versions).

Treat the specifications as the source of truth for observable behavior: which chunks are deleted or kept, safety guarantees, CLI options, exit codes, JSON shapes, and acceptance criteria. Internal design (types, APIs, module internals, algorithms, parallelism) is free to choose; do not infer an implementation from the wording of the specifications. When behavior is unspecified, ask instead of guessing.

Keep the specifications declarative: describe behavior, external facts, and the failures that must be distinguished. Do not add type names, function signatures, or implementation steps to them.

# Code Independence from the Specifications

The code must be readable without opening the specifications.
- Do not reference the specifications from code, comments, rustdoc, or tests (e.g. `see output.md`, `per spec`, `scan.md「…」`).
- When a rule needs explaining, state the rule itself in the comment or rustdoc.
- User-facing identifiers such as `T-1`, `L-1`, `S-3`, or `V-5` are part of the output and may appear in code, but the code must define what each one means (for example, in the rustdoc of the type or variant that carries it).

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
