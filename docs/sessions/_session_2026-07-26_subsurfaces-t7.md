# Session summary — 2026-07-26 — Subsurfaces v1 (M2 T7, pulled to the front)

**Task:** M2 T7 (prompt 13) — make subsurfaces real: scene tree, sync/desync
semantics, tree-aware damage and input, the T0 tripwire test inverted, foot
decorated.

**Build/test result:** `make test` — **124 tests green** (up from T0's 110, +14),
clippy clean, zero warnings. Five new goldens, blessed and demonstrated able to
fail. The two subprocess suites ran 8× consecutively without a flake after one
race in my own test was fixed.

**The headline:** `foot` renders **with its decorations**, and the acceptance test
asserts they composite. The debt opened by T7b's bad measurement and pinned by T0
is discharged.

---

## What was built

**The scene owns a tree; the renderer did not change.** Not one line of the
compositor. The snapshot still hands it a flat back-to-front list — the tree is
flattened on the scene thread, which is where the semantics belong (I-5).

Two decisions carry the design:

- **A child's transform is parent-relative.** Absolute position is the accumulated
  offset down the chain. Moving a parent carries its whole subtree without any
  child's stored state changing.
- **The children list contains the parent's own id** as the marker for its place
  in the stack. Not a borrowed trick — it is the only representation that can
  express `place_below`, where a child sits *beneath* its parent.

**Mapping law, extended:** a node composites iff it has a role, a source, a
non-empty size, **and every ancestor does too**. The T5 rule follows the tree down:
foot's pixel-less border subsurface composites nothing and is click-transparent.

**Sync/desync:** a synchronized child's commit returns immediately (Smithay caches
it); the effective commit walks the whole subtree and produces **one**
`SurfaceUpdate` list and **one** scene message. Atomicity is structural, not
best-effort — no snapshot can land between a parent's new content and its
children's.

**Input through the tree**, rebuilt on the dispatch side so routing never waits on
the scene (I-2). Subsurfaces are hit-testable but never keyboard-focusable.

## Three findings worth the reading

1. **76% of the output, from a missing equality check.** A subsurface's position is
   re-stated on *every* effective parent commit (that is how the protocol defers
   it), so damaging unconditionally repainted every decoration on every keystroke:
   365 354 px instead of 2 964. Correct output, ruinous cost, invisible without
   counters — the acceptance test's damage assertion caught it within a minute of
   subsurfaces first working. `set_subsurface_position` is now a no-op when the
   position is unchanged, and the doc says why.
2. **A `HashMap` made keyboard focus non-deterministic.** Input routing assigns
   each surface a stacking index while walking `toplevels` — iterating a HashMap
   meant "topmost" depended on hash order, and a two-window clipboard test began
   failing about half the time. The scene has always broken z-ties by `SurfaceId`;
   the routing table now does the same, so input and pixels cannot disagree about
   which window you are typing into.
3. **Two of my own tests were wrong, instructively.** One injected pointer motion
   after `commit()` without a round-trip (commit only queues, so the motion
   overtook the tree it was routed against); the other held a button down and
   expected the pointer to cross surfaces, when the implicit grab holding focus is
   correct protocol behaviour. Both were the compositor being right and the test
   asking the wrong question.

## Tests

| Suite | What lands |
|---|---|
| `conformance.rs` | **The inversion** (`a_subsurface_composites_its_content` — same test as T0's, opposite assertion); sync waits for the parent; desync applies immediately; `place_below` puts a child under its parent; unmapped-parent chain; the pixel-less case |
| `subsurface_render.rs` (new) | Goldens: child above, child below (peeking past the parent's edge), nested child-of-child with accumulated offsets, and the **sync-atomicity pair** — both frames pinned, before and after the parent's commit |
| `damage.rs` | The equivalence oracle extended: map child, map grandchild, move parent, move child, restack below parent, atomic batch, unmap parent — incremental == from-scratch throughout |
| `input.rs` | Click lands on the child with **child-local** coordinates; crossing back to the parent; a subsurface never takes keyboard focus; a pixel-less subsurface is click-transparent |
| `acceptance.rs` | **foot's decorations composite** — at least one mapped subsurface in its scene tree, waited for rather than sampled |
| `core::input` | A subsurface takes pointer input but not keyboard focus (unit) |

Golden discrimination re-demonstrated: a one-pixel error in accumulated child
offset is rejected with artifacts (128–144 differing pixels across the pair).

## Files changed

### Created
- `crates/harness/tests/subsurface_render.rs` — 4 golden tests (5 goldens).
- `crates/harness/goldens/subsurface_{above,below,nested,sync_before_parent_commit,sync_after_parent_commit}.png`
- `docs/sessions/_session_2026-07-26_subsurfaces-t7.md` — this summary.

### Modified — core
- `scene/node.rs` — `NodeRole::Subsurface`, `SubsurfaceRole`, `parent`, `children`.
- `scene/state.rs` — `MAX_SUBSURFACE_DEPTH`, `absolute_offset`, `node_rect`,
  `is_mapped`, `attach_subsurface`, `set_child_order`, `set_subsurface_position`,
  `detach_subsurface`, subtree damage, `SurfaceUpdate`, `apply_commit`, tree-aware
  `snapshot`, `composition_order`.
- `input.rs` — `FocusEntry::focusable` (subsurfaces route pointer, never keyboard).
- `protocol.rs` — `new_subsurface` hook; the commit path restructured around
  effective commits (`collect_tree`, `collect_surface`, `ordered_children`,
  `post_commit_bookkeeping`); `refresh_input_routing` + `flatten_for_input`.

### Modified — docs
- `docs/plans/m2_tasks.md` — the reorder recorded, with its three reasons.
- `docs/scene_graph_v1.md` — §12.3 replaced: the tree, mapping law, sync
  semantics, damage, flattening, and the two measured traps.
- `docs/parhelion_decision_log.md` — the tree decision + the **closing coda** on
  the T7b→T0→T7 arc; Pending item struck.
- `docs/diary.md` — "the tree, and the debt that took three sessions to pay".

### Project map
- `subsurfaces` node → `done` with real parts; `updated` bumped; `node --check`
  clean.

## Interactive checklist — please run this one

```bash
cargo run -p parhelion-backend-winit --bin parhelion-dev     # terminal 1
WAYLAND_DISPLAY=wayland-N foot                                # terminal 2
```

1. **foot now has decorations** — title bar and borders, drawn by foot into
   subsurfaces we finally composite. This is the new thing; everything below has
   been waiting politely since M1.
2. Type in foot: glyphs render, echo feels instant.
3. Clipboard: `echo hi | wl-copy` then `wl-paste` against the same display, and
   copy/paste inside foot.
4. **Resize the compositor window** — the placeholder panel stays put, background
   grows, no tearing or stale strips. *(Still unverified by me — needs eyes.)*
5. **Move the cursor over the window** — the host cursor shows, nothing flickers.
   *(Also still eyes-only.)*
6. Ctrl-C leaves no socket litter in `$XDG_RUNTIME_DIR`.

Items 4 and 5 are the two M1 leftovers I have never been able to check; the rest
is covered by tests.
