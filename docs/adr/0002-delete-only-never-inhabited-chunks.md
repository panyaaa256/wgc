# ADR-0002: Delete only never-inhabited chunks

- Status: Accepted
- Date: 2026-11-09

## Context

Recorded retroactively from the original specification.

Much of a world's size can be terrain that was generated around
players' paths but never visited. The game already records
how long players have been near each chunk: in vanilla, `InhabitedTime`
grows every tick for each chunk with a non-spectator player within 128
blocks. A chunk with `InhabitedTime = 0` has never had a player near
it, so deleting it loses no player activity; the game generates the
chunk again when a player first comes near.

Other signals (player traces such as placed blocks, a minimum time
threshold, areas the user wants to protect) need heuristics or
configuration, and each one is a way to delete something a player
cares about.

## Decision

- A region chunk is a deletion candidate only when its `InhabitedTime`
  tag is present, has the expected type, and its value is exactly 0.
- Anything wgc cannot decide is kept: a missing tag, a tag of the wrong
  type, a negative value, and a chunk that cannot be decompressed or
  parsed. A chunk in a region triple with a structural error is kept
  too ([0008](0008-copy-broken-region-triples-verbatim.md)).
- Further rules only remove candidates, never add them: forced-loaded
  chunks and their surroundings, and structure starts referenced from
  kept chunks, are protected. The full rule set is in
  [../gc-rules.md](../gc-rules.md).
- `entities` and `poi` entries follow the region chunk at the same
  position.
- `scan`, the estimate, and `gc --out` apply the same rules and give
  the same result for the same input.

## Consequences

- The criterion is only as good as `InhabitedTime` itself. Server
  settings and bugs that stop it from growing are checked before
  writing ([0007](0007-gate-output-on-trust-checks.md)).
- A regenerated chunk may differ from the deleted one when the world
  generator changed since it was first generated. wgc reports where
  deleted chunks border kept chunks that carry `blending_data`, but
  does not prevent the seam.
- Chunks a player only passed by keep a small positive value and are
  never deleted; wgc reclaims less than a more aggressive tool would.
- Out of scope for the MVP: judging by player traces, judging by the
  history of server types, user-defined protection areas, and spawn
  chunk protection. Threshold-based deletion is deferred.
