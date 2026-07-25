# Prompt 13 — Subsurfaces v1 (M2 T7, pulled to the front)

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-25.
**Milestone:** M2 — the task labeled T7 in `docs/plans/m2_tasks.md`,
executed second. Step 0 records the reorder.
**Reads first:** the T0 session summary and CORRECTION entry (this
task's origin story); `docs/plans/m2_tasks.md` T7; `docs/
scene_graph_v1.md` (node model, damage, mapping semantics).

---

## Step 0 — Record the reorder

Add a dated note to `m2_tasks.md`'s ordering section: T7 executes
after T0, before T1, because (a) the T0 correction revealed the
subsurface gap is *live* — foot renders undecorated, silent wrongness
shipping since M1; (b) the work needs no DRM/GPU and is easier
against tolerance-0 CPU goldens and the equivalence oracle than after
T5 loosens them; (c) the pinned wrong-behavior test inverts sooner.
Small scoped change; index untouched.

## Context

foot creates nine subsurfaces and commits buffers into eight — its
decorations — and Parhelion drops them on the floor. This task makes
subsurfaces real: scene child nodes, sync/desync commit semantics,
tree-aware damage and input. Smithay's compositor module already
tracks the subsurface tree, cached states, and relative locations;
the work is mapping that faithfully into our scene, not reinventing
the protocol.

## Design constraints

1. **The scene grows a tree, carefully.** Nodes gain children:
   parent-relative position, sibling ordering from
   `place_above`/`place_below` (relative to parent or siblings, per
   protocol), arbitrary nesting depth (subsurfaces of subsurfaces —
   Smithay's traversal handles the tree; our scene must not assume
   depth 1). The snapshot flattens the tree to composition order —
   the renderer stays a back-to-front list consumer; the *scene*
   owns tree semantics. Born-3D-ready discipline unchanged: child
   transforms are parent-relative translations today, slots tomorrow.
2. **Mapping law extends:** a subsurface is a role; it is mapped iff
   it has a committed buffer AND its parent is mapped (transitively).
   foot's pixel-less ninth subsurface is the test case nature
   provided: role assigned, no buffer, never composites, never takes
   input. The T5 rule ("what cannot be seen cannot be clicked")
   applies through the tree.
3. **Sync/desync per protocol, exactly:** sync children's state
   (buffer, damage, position) is cached and applied atomically at the
   nearest desync ancestor's commit; desync applies immediately;
   `set_position` takes effect on parent commit in both modes; mode
   changes take effect per spec. Smithay's cached-state machinery is
   the substrate — our job is applying it to the scene at the right
   commit, atomically (one scene message per effective commit, not
   one per surface — the atomicity is user-visible).
4. **Damage through the tree:** child damage translates through
   accumulated parent offsets to output space; position changes and
   restacks are structural damage (old ∪ new); the equivalence
   oracle's scripted sequence gains subsurface steps (sync-commit
   atomicity is exactly the kind of thing incremental rendering gets
   wrong — the oracle is the guard).
5. **Input through the tree:** hit-testing walks children above
   parents in their stacking order; surface-local coordinates are the
   child's own; focus and the clipboard's focus gate work unchanged
   when the focused surface is a subsurface's parent (subsurfaces
   never take keyboard focus per protocol — pointer only).

## Task

1. Scene tree + snapshot flattening; mapping law; sync/desync
   application; damage translation; input hit-testing.
2. Protocol wiring: Smithay subsurface hooks into scene messages;
   the T0 conformance test that pins buffer-dropping **inverts** —
   subsurface content now composites (this is the tripwire retiring
   itself, as designed).
3. Tests:
   - Goldens: parent with child above; child below; nested
     (child-of-child) offset composite; sync-atomicity golden pair
     (child committed, parent not → old frame; parent commits → new
     frame, both pinned).
   - Conformance: sync vs desync timing; set_position deferral;
     place_above/below reordering; unmapped-parent chain (map parent
     → whole tree appears; unmap → tree vanishes, damage correct);
     the pixel-less-subsurface case.
   - Equivalence oracle: extended sequence with subsurface commits,
     moves, restacks, sync batches — incremental == scratch
     throughout.
   - Input: click lands on child over parent; coordinates
     child-local; pixel-less child is click-transparent.
   - **foot decorated:** the acceptance test asserts foot's scene
     tree contains mapped subsurface nodes (≥1) and that decoration
     pixels composite (a border-region pixel-change assertion at
     map time — counters/pixels, not goldens, fonts rule unchanged).
4. Docs: `scene_graph_v1.md` — tree section (mapping law, sync
   semantics, flattening, damage translation); decision log — the
   inversion coda on the CORRECTION chain (the debt's discharge,
   closing the T7b→T0→here arc); diary (`#scene` `#protocol`; the
   narrative writes itself); session summary; map — subsurface seam
   → done, `updated`, `node --check`.
5. Interactive: the smoke checklist now includes "foot has
   decorations" — plus the two still-open M1 eyes-items (resize,
   cursor-over-window) which have waited politely; ask Roland to run
   the smoke after this lands.

## Acceptance

- `make test` green (110 + the suite above); clippy clean; CI green.
- The formerly-pinned wrong behavior test inverted, not deleted —
  its comment tells the story.
- Equivalence oracle green with tree steps; sync atomicity golden
  pair both pinned.
- foot acceptance asserts decorations present.
- No renderer structural change (still a flat back-to-front list);
  no popups; no keyboard focus for subsurfaces.

## Out of scope

Popups/positioners (standing note stands); viewporter/scale on
subsurfaces; DnD; everything T1+ on the metal path. If Smithay's
cached-state application resists the one-message-per-effective-commit
atomicity, stop and report — atomicity is the semantic heart here
and not negotiable by workaround.
