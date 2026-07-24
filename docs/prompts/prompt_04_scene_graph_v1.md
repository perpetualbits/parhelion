# Prompt 04 — Scene graph v1, thread skeleton, first composition

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-25.
**Milestone:** M1, task T1 (see `docs/plans/m1_tasks.md` — install it
from `downloads/` as step 0 of this session, and index it).
**Reads first:** `docs/plans/m1_tasks.md`; `CORE-BOUNDARY.md` §7 and
C4/C5; `VISION.md` Theses 1 & 3; `docs/scene…` — does not exist yet;
you create it this session.

---

## Step 0

Install `m1_tasks.md` from `downloads/` to `docs/plans/m1_tasks.md`;
add it to the project index.

## Context

Everything in M1 hangs off this task: the canonical scene state, the
§7 thread skeleton at shards = 1, the snapshot mechanism, and a CPU
renderer v1 that composites the first real frames. The M0 scene ledger
is absorbed: `ProtocolHost` now publishes to the real scene owner, and
the ledger's rig tests migrate into scene-state assertions.

## Design constraints (the narrow line)

1. **Born 3D-ready, implemented 2.5D.** A scene node carries a
   transform slot and a texture-source binding from day one (Thesis 1),
   but only the axis-aligned integer-position path is implemented,
   composited, or tested in M1 (Thesis 3). Concretely: the transform
   type exists and defaults to identity/translation; any non-trivial
   transform is `unimplemented!`-class rejection at this stage, not a
   half-working code path. Building 3D now is scope creep; building
   types that forbid 3D is a Thesis-1 violation. When in doubt on this
   line, ask.
2. **Texture sources are the Rayland seam.** An extensible binding
   (enum or small trait — your call, module-doc the choice) with
   exactly two members now: `Solid(color)` for tests, and a declared
   placeholder for `Shm` (T3 implements it). The module doc names the
   future members (dmabuf, Rayland token-buffer per CORE-BOUNDARY C9)
   and states that *nothing in the scene or renderer may assume pixels
   are locally produced* — that sentence is the seam.
3. **Ownership per §7.** Scene canonical state is owned by one thread;
   `ProtocolHost`'s dispatch thread reaches it only via the existing
   message channel (grown as needed); T-render consumes **immutable
   snapshots**. Snapshot v1 is a straightforward full copy of the
   (small) visible-node list — CORE-BOUNDARY open question §10.3
   (persistent structural sharing) stays open; leave a doc note, not a
   premature abstraction.
4. **Renderer v1 is a CPU compositor**, evolving the M0 test-pattern
   code: iterate snapshot nodes back-to-front, fill/blit into a
   `Frame`. Deterministic, integer-only, tolerance-0 goldens. No GPU,
   no Smithay renderer types (decision log), no damage yet (T4).

## Task

1. Scene state + node types in `crates/core` (new module or crate —
   your call against CLAUDE.md's layout; flag if you add a crate).
2. Scene thread owning canonical state; protocol→scene messages for
   surface lifecycle (absorbing ledger semantics); scene→render
   snapshot channel; T-render skeleton driving the CPU compositor at a
   test-controlled tick (no wall-clock in tests).
3. CPU compositor v1 over snapshot nodes (solid sources only).
4. Migrate the M0 ledger rig tests to scene-state assertions; keep the
   protocol rig green throughout.
5. Golden tests: two overlapping solid nodes (stacking order visible);
   restack changes the golden; out-of-bounds/clipped node handled.
6. Snapshot-isolation test: take a snapshot, mutate canonical state,
   render the snapshot — output unaffected.
7. Frame instrumentation skeleton: per-frame counters (frames
   produced, nodes composited) — the counter *mechanism* T4 will
   extend with damage metrics; keep it minimal.
8. `docs/scene_graph_v1.md` — short canonical doc: node model and the
   3D-ready/2.5D-implemented line, texture-source seam sentence,
   thread ownership diagram (ASCII fine), snapshot semantics and the
   §10.3 note, what T2/T3/T4 will add. Update CLAUDE.md's subsystem
   table; index it.
9. Session summary; diary (`#core` `#scene` + earned tags); `make
   test` result stated.

## Acceptance

- `make test` green: all prior tests (migrated where the ledger died)
  plus the new golden and isolation tests; clippy clean.
- Thread ownership per §7 visible in code and stated in the doc — a
  reviewer can point at which thread owns what and where messages
  cross.
- The texture-source seam exists with its module-doc sentence; grep
  for Smithay renderer types in `crates/core` finds nothing.
- Goldens tolerance-0; blessing workflow unchanged.
- No transform math beyond identity/translation is reachable.

## Out of scope

Frame callbacks and flush ownership (T2); shm (T3); damage (T4);
xdg-shell (T5); input and winit (T6); any 3D math; `presentation-time`;
persistent snapshot sharing (§10.3 stays an open question).
