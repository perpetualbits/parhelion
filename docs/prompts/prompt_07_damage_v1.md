# Prompt 07 — Damage tracking v1

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M1, task T4 (see `docs/plans/m1_tasks.md`).
**Reads first:** `docs/plans/m1_tasks.md` T4; `docs/scene_graph_v1.md`
§3.1 (partial-copy note) and the snapshot section; `VISION.md` Thesis 3
(the 2.5D regime this task begins to earn).

---

## Context

Until now every tick recomposites everything. This task makes the
compositor stop doing work: per-surface damage accumulates per
protocol, flows through the scene into the snapshot as an output-space
region, and the renderer recomputes only damaged pixels against a
retained frame. Counters make the savings measurable — they are the
evidence for M1's acceptance and, eventually, M5's benchmarks.

## The governing property

**Incremental must equal from-scratch.** For any sequence of commits
and scene changes, rendering incrementally with damage produces a
frame byte-identical to discarding the retained frame and compositing
from nothing. Damage may only ever change *cost*, never *output*. This
property gets its own test (below) and it is the test that matters
most in this task; everything else is plumbing around it.

## Design constraints

1. **Conservative regions.** Damage representation is a small owned
   region type: a rect list with union/intersect/translate and a
   coalescing rule (when the list exceeds N rects, collapse toward
   bounding boxes — N a named constant with reasoning).
   Over-approximation is always legal; under-approximation is a bug
   class. Do not import a region crate or Smithay's desktop-layer
   region handling; `smithay::utils` geometry types are fine per the
   layer table. Keep subtraction out unless something genuinely needs
   it — it is where region code grows teeth.
2. **Protocol semantics.** `wl_surface.damage` (surface coords) and
   `wl_surface.damage_buffer` (buffer coords) both accumulate,
   double-buffered, applied at commit. With scale/transform out of
   scope, buffer and surface coordinates coincide — state that
   assumption where the two are merged, so the M2+ generalization has
   a marked site. Damage clips to the surface extent.
3. **Scene-side damage.** Frame damage is the union of: client damage
   translated to output coordinates; structural changes (node
   moved/resized → old ∪ new extent; restack → affected extent;
   map/unmap/source-change → extent); and full-output damage for the
   first frame and any "don't know" case — the explicit conservative
   fallback, counted separately so its frequency is visible.
4. **Retained-frame rendering.** T-render keeps its previous output;
   per tick it composites only within the snapshot's damage region
   (per damage rect: all intersecting nodes, back-to-front, clipped).
   Nodes wholly outside damage cost nothing. (Opaque-region occlusion
   culling remains M5 — do not start it.)
5. **Damage-aware partial copy** (the §3.1 pickup): at commit, copy
   only the damaged region of the buffer into the surface's pixel
   block. Since in-flight snapshots share that block by `Arc`,
   partial copy means copy-on-write — never mutate shared pixels;
   clone-then-patch (or `Arc::make_mut` semantics) with the fresh
   full-copy path as the fallback when there is no prior block or
   damage covers everything. This extends snapshot isolation down to
   pixel level, and it gets a dedicated test.
6. **Counters** (extending T1's skeleton): pixels redrawn per frame,
   damage rects per frame, full-damage frames, bytes copied at
   commit. Documented in the scene doc; asserted in tests — counters
   nobody asserts on drift into lies.

## Task

1. Region type + unit tests (union, translate, clip, coalesce
   threshold behavior — including the pathological many-small-rects
   case).
2. Protocol damage accumulation → commit application → scene messages
   now carrying damage.
3. Scene damage computation (constraint 3) into the snapshot.
4. Retained-frame renderer honoring snapshot damage.
5. Partial copy with CoW (constraint 5).
6. Tests:
   - **Equivalence test:** a scripted multi-step sequence (commits
     with small damage, a move, a restack, a source change, a full
     redraw) rendered incrementally vs from-scratch at every step —
     byte-identical throughout. Make the sequence awkward on purpose
     (overlaps, damage spanning two nodes, damage on an occluded
     area).
   - Proportionality: a small-damage commit on a large surface
     redraws pixels within a stated bound of the damage area (assert
     via counters, with the bound generous enough to allow rect
     granularity — named constant, reasoning).
   - Coalescing fallback: many scattered rects → collapses, output
     still correct (equivalence check again).
   - CoW isolation: snapshot taken, partial copy commits after it —
     the in-flight snapshot's pixels are unaffected, byte-checked.
   - All existing goldens green untouched — damage must not change
     any of them.
7. Docs: `scene_graph_v1.md` damage section (region rules, coalescing,
   conservative fallbacks, counters, the marked buffer==surface
   assumption); decision-log entry if any call rises to load-bearing
   (the coalescing policy likely does); diary; session summary;
   `make test` stated.

## Acceptance

- All prior tests green plus the suite above; clippy clean.
- The equivalence test exists, covers structural changes as well as
  client damage, and fails if the renderer is made to skip honoring a
  damage class (verify by temporary sabotage, then revert — state it).
- Proportionality demonstrated by counters, not by eye.
- No pixel of a shared block is ever mutated (CoW test).
- Conservative-fallback counter observable; first frame counts as one.

## Out of scope

Opaque-region occlusion culling and plane offload (M5); damage
forwarded to KMS (M2/M5); buffer scale/transform/viewporter;
occlusion-aware callback throttling (M2); subsurfaces; any change to
the flush/backpressure machinery.
