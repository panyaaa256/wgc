# ADR-0007: Gate output on trust checks

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

The deletion rule assumes that `InhabitedTime = 0` means no player was
ever near the chunk ([0002](0002-delete-only-never-inhabited-chunks.md)).
Some servers break that assumption, and on those worlds wgc would
delete chunks players actually used. Several of these cases can be
detected from files in the world or server folder.

## Decision

Before writing, `gc --out` runs these checks over the targeted
dimensions. Each one passes, fails, cannot be checked, or does not
apply:

| ID | Stops when |
|---|---|
| T-1 | Paper's `chunks.fixed-chunk-inhabited-time` is 0 or more, which pins every chunk's value |
| T-2 | Mob spawning is disabled on a Bukkit world at 1.21.5 or later, where Paper does not increase `InhabitedTime` (PaperMC/Paper#12827) |
| T-3 | A non-spectator player stands in a chunk whose `InhabitedTime` is 0 |
| T-4 | The world's DataVersion is newer than wgc knows |
| T-5 | The forced-loaded chunks of a targeted dimension cannot be read |

L-1 (`session.lock` is missing,
[0005](0005-lock-session-lock-like-the-game.md)) is handled the same
way.

- A failure or "cannot be checked" stops `gc --out` before anything is
  written. Unreadable files never become errors; they make the check
  "cannot be checked".
- `--force` overrides every stop; `--force=T-3,T-5` overrides only the
  listed IDs. Overridden stops are warned about and recorded in
  `forced` in `complete.json`, and `wgc verify` reports them.
- Stops not overridden are detected before region files are read, so
  a refused run is fast. The checks are repeated on the finished plan
  and must agree.
- `scan` and the estimate show every result and never stop.

## Consequences

- Users on affected servers must decide explicitly, per ID, to proceed.
- T-2 also stops Spigot and other non-Paper servers, which do not have
  the bug, because the files do not tell them apart.
- Overriding T-5 deletes without forced-chunk protection.
- The checks catch only known causes. A plugin or fork that changes
  `InhabitedTime` in an undetectable way is not covered.
- T-4 means every new Minecraft release needs a wgc update before it
  can run without `--force`. Details are in
  [../gc-rules.md](../gc-rules.md).
