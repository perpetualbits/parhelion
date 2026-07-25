# Session summary — 2026-07-25 — Damage tracking v1 (M1 T4)

**Task:** M1 T4 (prompt 07) — per-surface damage accumulates, flows through the
scene into the snapshot as an output-space region, and the renderer recomputes
only damaged pixels against a retained frame. Counters make the savings
measurable. The governing property: **incremental rendering byte-identical to
from-scratch.**

**Build/test result:** `make test` — **56 tests green** (up from T3's 47, +9:
5 region unit + 4 damage integration), clippy clean. All existing goldens
untouched (damage changes cost, not output). The equivalence test was verified to
**fail under sabotage** (dropping `set_z`'s damage → the restack step mismatches),
then reverted.

## Files changed

### Created
- `crates/core/src/scene/region.rs` — `Rect`, `Region` (union/translate/
  intersect/clip, coalesce-to-bbox past `MAX_DAMAGE_RECTS = 16`), 5 unit tests.
- `crates/harness/tests/damage.rs` — `incremental_equals_from_scratch` (the
  governing property), `small_damage_redraws_a_small_region` (proportionality via
  counters), `many_scattered_rects_coalesce_and_stay_correct`,
  `partial_copy_does_not_mutate_in_flight_snapshot` (CoW isolation, byte-checked).
- `docs/sessions/_session_2026-07-25_damage-v1-t4.md` — this summary.

### Modified
- `crates/core/src/scene/mod.rs` — export `Rect`/`Region`/`MAX_DAMAGE_RECTS`,
  `SnapshotDamage`, `ContentDamage`; add `region` module.
- `crates/core/src/scene/snapshot.rs` — `SnapshotDamage::{Full, Region}`;
  `Snapshot.damage` field.
- `crates/core/src/scene/state.rs` — `Scene` gains `pending_damage`/`full_damage`;
  setters damage as they mutate (old ∪ new extent); new `attach_content`
  (content-vs-structural split), `set_size`/`clear_source` damage, `damage_full`;
  `snapshot(&mut self)` drains damage; `ContentDamage` enum; `node_output_rect`.
- `crates/core/src/render.rs` — `Compositor::composite -> CompositeStats`;
  `FrameCounters` gains `pixels_redrawn`/`damage_rects`/`full_damage_frames`;
  `tick` folds stats + counts damage class.
- `crates/backend-headless/src/composite.rs` — retained-frame rendering:
  `paint_rect` (clear + draw intersecting nodes per damage rect), `clear_rect`,
  `blit_solid`/`blit_pixels` take a clip rect; `Full` vs `Region` paths; off-screen
  nodes skipped (one test assertion updated to reflect the culling).
- `crates/core/src/protocol.rs` — `commit` reads client damage + does damage-aware
  partial copy (`build_pixel_block`, CoW via `Arc::make_mut`); `damage_to_rects`
  (the marked buffer==surface site); `State` holds `surface_pixels` (retained
  blocks) + `bytes_copied`; `ProtocolHost::bytes_copied`; prune retained blocks
  with dead surfaces.
- `crates/harness/src/protocol_rig.rs` — `ScriptedClient::damage` (post
  `wl_surface.damage`).
- `docs/scene_graph_v1.md` — new §9 (damage tracking: property, region rules,
  scene-side damage, retained rendering, partial-copy CoW, counters); old §9 →
  §10; §7 tests + header updated.
- `docs/parhelion_decision_log.md` — 2026-07-25 T4 section: coalescing policy
  (load-bearing) + content-vs-structural / retained-frame / CoW.
- `docs/diary.md` — M1 T4 narrative (the one property, the two bugs, the split,
  no-subtraction, CoW-is-isolation, sabotage).
- `docs/parhelion_project_index.md` — current state (T4 complete, 56 tests).
- `project-map.js` — damage node + scene-graph damage part → done (per the
  standing project-map rule).

## Invariants touched (cited in code)

- **I-9** (2.5D damage-tracked regime — this seeds it).
- **I-1** — snapshot stays an owned, lock-free copy; the partial copy is off the
  frame path; damage only shrinks the compositor's work.
- Snapshot isolation extended to the pixel level (CoW).

## Acceptance (prompt 07)

- All prior tests green + the new suite (56 total); clippy clean. ✓
- Equivalence test exists, covers structural + content damage, and fails under
  sabotage (verified, reverted). ✓
- Proportionality shown by counters, not by eye. ✓
- No pixel of a shared block ever mutated (CoW test). ✓
- Conservative-fallback counter observable; first frame counts as one. ✓

## Notes / follow-ups (later)

- Opaque-region occlusion culling and plane offload (M5).
- Damage forwarded to KMS (M2/M5); buffer scale/transform/viewporter (the marked
  buffer==surface site generalizes then).
- True in-place patch (skip the make_mut clone when unshared is common) — a later
  optimization; M1 clones every commit (correct, cheap enough).
