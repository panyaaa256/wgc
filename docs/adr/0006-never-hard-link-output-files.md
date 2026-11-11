# ADR-0006: Never hard-link output files

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

Most files in the output are identical to the original: level data,
player data, and region files that lose no chunks. Hard-linking them
would make the output nearly free in time and disk space.

Minecraft, however, overwrites region files in place rather than
writing a new file and renaming it. After the swap, the game writing
to a hard-linked output file would write to the same inode as the
original world, and the original is the only restore path
([0001](0001-never-modify-the-source-world.md)).

## Decision

- Every output file is a real copy. wgc never creates hard links.
- Symbolic links in the source are followed and their targets are
  written as regular files, with a warning.
- Permission bits and modification times are copied from the original;
  owner and group too when running as root.

## Consequences

- The output needs disk space and time for a full copy of everything
  it keeps, including files wgc does not change.
- After the swap, the output and the original share nothing, so using
  the new world can never change the backup.
- Reflink (copy-on-write) copies would save space without this
  problem on filesystems that support them; they are deferred.
