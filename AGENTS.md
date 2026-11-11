# Project Context

This workspace contains wgc, a chunk GC for Minecraft Java Edition worlds written in Rust. It deletes chunks that no player has ever been near (`InhabitedTime = 0`) and writes the result to a new folder, never modifying the original world.

# Reference Documentation

Read before generating, refactoring, or reviewing code:
- **`ARCHITECTURE.md`**: Code map, module boundaries, invariants, and cross-cutting concerns.
- **`docs/gc-rules.md`**: What is deleted, kept, checked, and verified. The source of truth for deletion and safety behavior.
- **`docs/world-format.md`**: External facts about Minecraft, Bukkit, and Paper storage formats, with sources.
- **`docs/json.md`**: The JSON output contract, pinned by golden tests.
- **`README.md`**: CLI options, output conventions, and exit codes.
- **`CONTRIBUTING.md`**: Workspace, checks, testing, documentation, and commit conventions.
- **`docs/adr/`**: Why major decisions were made. ADRs are historical records of rationale, not instructions; when an ADR and a reference document disagree, the reference document describes current behavior.

The documents define **what** wgc does, not **how** it is built. Internal design (types, APIs, module internals, algorithms, parallelism) is free to choose; do not infer an implementation from their wording. When behavior is unspecified, ask instead of guessing. Keep the documents behavioral: do not add type names, function signatures, or implementation steps to them. Two exceptions: `ARCHITECTURE.md` names modules and key types because it is a code map, and ADRs may name implementation choices because that is what they record.

# Code Independence from the Documentation

The code must be readable without opening the documents.
- Do not reference the documents from code, comments, rustdoc, or tests (e.g. `see gc-rules.md`, `per spec`).
- When a rule needs explaining, state the rule itself in the comment or rustdoc.
- User-facing identifiers such as `T-1`, `L-1`, `S-3`, or `V-5` are part of the output and may appear in code, but the code must define what each one means (for example, in the rustdoc of the type or variant that carries it).

# Code Quality & Refactoring Directives

- **Concise & Idiomatic Rust**: Write clean, modern, and idiomatic Rust. Prefer simplified expressions, standard combinators (e.g., `Option`/`Result` combinators), and clean control flow over overly verbose logic.
- **Signal-to-Noise Ratio in Comments**:
  - **Avoid Redundant Comments**: Do not write comments that merely restate what the code clearly expresses (e.g., `// create a new vector`, `// return the result`).
  - **Keep Value-Additive Comments**: Retain or add comments only when explaining complex algorithm choices, subtle invariants, safety guarantees (`// SAFETY:`), or architectural context.
- **Self-Documenting Code**: Prefer expressive function, variable, and type names over heavy comment explanations.

# Code Generation Directives

- Respect crate boundaries inside `crates/*` and adhere strictly to the unidirectional dependency flow defined in `ARCHITECTURE.md` (`cli` -> `core`).
- Never use `unwrap()`, `expect()`, or `panic!` in non-test library code.
- Write code comments, rustdoc, commit messages, and all documentation in English. `README_ja.md` is the only exception: a Japanese translation of `README.md`, kept in sync with it.

# Commit Directives

- Write commit messages in the [Conventional Commits](https://www.conventionalcommits.org/) format: `<type>[optional scope]: <description>`.
  - **Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
  - **Scope**: the affected area, such as a crate or module (`core`, `cli`, `region`, `output`, `docs`).
  - **Description**: imperative mood, present tense, under 72 characters.
  - Mark a breaking change with `!` after the type or scope, or with a `BREAKING CHANGE:` footer.
- Keep one logical change per commit.
