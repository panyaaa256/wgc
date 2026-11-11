# ADR-0005: Lock session.lock like the game

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

Copying a world while the game is running produces an inconsistent
output, and swapping one in while the game runs loses data. Since
20w14a the game takes an OS lock on `<world>/session.lock` at startup
through Java's `FileChannel.tryLock`. On Unix that is an `fcntl` record
lock; on Windows it is `LockFileEx`.

On Linux, `flock` locks and `fcntl` locks do not interact, so an
`flock` on `session.lock` neither sees the game's lock nor blocks the
game. Rust's `std::fs::File::try_lock` and the `fs2`/`fs4` crates use
`flock` on Unix, so none of them work here.

## Decision

- wgc takes the same kind of lock the game takes, without waiting,
  exclusive, over the whole file:
  - Linux: an open file description lock (`fcntl(F_OFD_SETLK)`).
  - Other Unix: `fcntl(F_SETLK)`.
  - Windows: `LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK |
    LOCKFILE_FAIL_IMMEDIATELY`.
- wgc opens `session.lock` for writing without creating or truncating
  it, and never writes to it.
- A world made of several world folders (Bukkit) is locked in every
  folder, each folder once.
- `gc --out` holds the locks until writing, verification, and
  recording are done. While wgc holds them, the game cannot start on
  that world.
- A lock held by another process stops `gc --out`, makes `wgc verify`
  fail, and is only a warning for `scan` and the estimate. A missing
  `session.lock` is the stop L-1, which `--force` can override
  ([0007](0007-gate-output-on-trust-checks.md)).
- Exclusion against a running Java process is covered by a Linux
  integration test.

## Consequences

- Classic POSIX record locks are released when the process closes any
  descriptor of the same file. wgc therefore never opens `session.lock`
  for anything else while holding the lock (`stat` is fine), and
  refuses to lock a folder it already holds.
- Opening for writing needs write permission; a user who cannot write
  `session.lock` is told to run wgc as the server's user.
- Worlds from before 20w14a (DataVersion 2524) have no OS lock, so wgc
  cannot detect whether the game is running. It warns and relies on
  the user.
