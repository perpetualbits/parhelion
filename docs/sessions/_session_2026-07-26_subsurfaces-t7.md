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

> **`wayland-N` is a placeholder.** `parhelion-dev` prints the real display name
> it bound (`wayland-3`, say) and echoes a copy-pasteable `try …` line — use that
> number. It changes between runs, because the socket is bound to the first free
> slot.

```bash
cargo run -p parhelion-backend-winit --bin parhelion-dev     # terminal 1
WAYLAND_DISPLAY=wayland-3 foot   # terminal 2 — use the number printed above
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

---

## Smoke results — run by Roland, 2026-07-26

Verified on the dev machine, by eye, against `parhelion-dev` + real clients:

| Item | Result |
|---|---|
| **foot has a title bar** | ✅ Decorations composite — the subsurface work, confirmed visually |
| **A second, independent terminal (`rt`, Roland's own)** | ✅ Runs, has a title bar, "generally looks OK" |
| **`rt` launched *from inside* foot** | ✅ A second window appears, typeable, immediate — multi-window, focus handover and C10 cascade placement all working live |
| **Typing latency (both terminals)** | ✅ Immediate |
| **Resize the Parhelion window** | ✅ *(M1 leftover, open since T6 — now closed)* |
| **Cursor over the window** | ✅ No flicker *(M1 leftover, open since T6 — now closed)* |

**The two items no test could reach are now verified**, and the milestone's claim
has a second witness: `rt` was written by Roland against no knowledge of
Parhelion's internals, and it behaves like foot does. Two unrelated clients, one
launched by the other, both decorated, both responsive.

Roland's summary: *"I would call this a success."*

## Follow-up: the first window's decorations were off-screen

Roland's smoke found what the suite could not: **the first `foot` had no visible
decorations; every later window did.**

Measured cause — foot places its decorations outside its own surface and declares
exactly that:

```
wl_subsurface.set_position(0, -26)      # title bar, 26 px ABOVE the surface
wl_subsurface.set_position(-5, -26)     # borders, outside on every side
xdg_surface.set_window_geometry(0, -26, 696, 494)
```

We placed the **raw surface** at the C10 cascade slot, and the first slot is
`(0, 0)` — so the title bar landed at y = −26, off the top of the output. Later
windows get (32,32), (64,64)…, which have room. The decorations were composited
correctly the whole time, into pixels outside the screen.

**Fix:** placement now subtracts the declared geometry's origin, so the *window*
lands at the cascade slot and the decoration overhang falls outside it — what
`set_window_geometry` is for, and what every real compositor does with it.

**Test:** `a_window_is_placed_by_its_declared_geometry_not_its_surface_origin`,
verified to fail against the old placement (`(0,0)` where `(4,6)` is required).

**Worth keeping:** no test in the suite could have found this. Every rig client
draws at its own origin and declares no geometry, so none of them had the shape
that fails. Our tests describe clients we write; the interesting failures come
from clients we don't.
