# ADR-0013: Keep the core crate no_std

- Status: Accepted
- Date: 2026-11-27

## Context

`wgc-core` used to hold everything below the CLI: the pure handling of
region files and NBT, but also file system access, locking, platform calls
and the steps of each command. Nothing stopped pure code from reaching for
the file system, and the CLI kept some decisions of its own (exit-code
classification, input validation, summary totals).

TMCJapan/sekai, whose layout detection wgc follows
([0009](0009-detect-layouts-like-sekai.md)), keeps every crate below its
binary `no_std` except the ones that touch the host, and hands the
operations file handles instead of paths.

## Decision

- The workspace is split by what a crate may touch:
  - `wgc-core` is `no_std` + `alloc`: value types, region files, NBT and
    decompression. It never sees a path.
  - `wgc-fs` holds everything that touches the host file system.
  - `wgc-cli` builds the `wgc` binary: argument parsing, every user-facing
    sentence, terminal output and exit codes.
  - A `wgc` library crate, also `no_std`, is to take over the steps of each
    command from `wgc-fs`, written against traits that `wgc-fs` implements.
- Crate names follow common Rust conventions: `-core` for the foundation,
  `-fs` for the host file system, `-cli` for the binary package.
- zlib and gzip are decoded with `miniz_oxide` and checksummed with
  `crc32fast`, both `no_std`. `flate2` with `zlib-rs` stays for tests only:
  `zlib-rs` needs `std` for its Rust allocator.
- CI checks `wgc-core` on a target without `std`, so a dependency that
  pulls `std` in fails the build.

## Consequences

- `no_std` makes the boundary a compiler check: code in `wgc-core` cannot
  read or write a file, print, or spawn threads.
- `miniz_oxide` inflates about 15-20% slower than `zlib-rs` in a
  micro-benchmark on chunk-sized zlib streams, and inflating chunks is most
  of the time spent reading a world.
- The gzip container (member headers, CRC-32 and size trailers, multiple
  members) is parsed by wgc itself, keeping the strictness of `flate2`'s
  `MultiGzDecoder`: an empty file, trailing bytes, reserved flags or a wrong
  checksum are all corrupt.
- Test builders for region files and NBT are shared with the other crates
  through the `test-util` feature of `wgc-core`.
