# ADR-0009: Detect layouts like sekai

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

wgc takes a world folder for vanilla and a server root for Bukkit-based
servers. Where the dimensions live depends on the server type and on
the version: 26.1 moved dimensions under `dimensions/<ns>/<name>`,
while older Bukkit servers split a world into `<base>`,
`<base>_nether`, and `<base>_the_end`. The sister project TMCJapan/sekai
already detects these layouts for the same users and the same servers.
Two tools from the same organization disagreeing about what a path
contains would be confusing, and for wgc a wrong answer means missing
dimensions.

## Decision

- Layout detection gives the same result as sekai's
  (`crates/world/src/tree/hostworld/`) for the four layouts: vanilla
  before and after 26.1, Bukkit before and after 26.1. This includes
  how `level-name` is read from `server.properties` and the order in
  which candidates are tried.
- On top of detection, wgc reports likely mistakes: W-1 (a vanilla
  server root detected as Bukkit; wgc suggests `<root>/<base>`) and W-2
  (a Bukkit world folder passed as a vanilla world).
- Region files outside the detected dimension folders are not searched
  for; they are copied like any other file.

The detection rules are in [../world-format.md](../world-format.md).

## Consequences

- Users get the same answer from both tools, and fixes found in one
  can be checked against the other.
- wgc does not depend on sekai's code; it reimplements the rules, so
  the two can drift. Changes in sekai's detection have to be followed
  manually.
- Server layouts sekai does not know are not supported either.
