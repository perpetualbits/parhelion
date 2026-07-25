# Parhelion — Scene Graph v1

> **Re-entrancy header.**
> **Status:** Draft v0.1 · **Date:** 2026-07-24 · **Kind:** subsystem design (the scene graph, render loop, and snapshot mechanism).
> **Upstream:** `VISION.md` Theses 1 & 3; `CORE-BOUNDARY.md` §3 (C4 scene graph, C5 render loop), §7 (threading), §10.3 (snapshot representation — open); `docs/plans/m1_tasks.md` T1.
> **Downstream:** T2 (frame callbacks / flush ownership / backpressure — landed, §8), T3 (wl_shm → real `Shm` source — landed, §3.1), T4 (damage tracking — landed, §9), T5 (xdg-shell, roles, and the mapping-semantics migration — landed, §10), T6 (seat/input, focus, and the nested winit backend — landed, §11).
> **Canonical for:** `crates/core/src/scene/`, `crates/core/src/input.rs`, `crates/core/src/render.rs`, the protocol frontend's scene/input edges in `crates/core/src/protocol.rs`, and the backends in `crates/backend-headless/` and `crates/backend-winit/`.
> **Change control:** this document is the single canonical scene-graph doc (CLAUDE.md: one doc per subsystem). It supersedes the M0 ledger's role; the ledger is gone.

---

## 1. What this is

The scene graph is Parhelion's **canonical state** (`CORE-BOUNDARY.md` C4, invariant I-5): the in-core truth about which surfaces exist, where they are, how they stack, and what textures them. Everything else — servers, the render side, future policy daemons — holds only views derived from here.

M1 T1 builds the smallest honest version of it: canonical scene state owned by one thread, immutable per-frame snapshots crossing to a render skeleton, and a CPU compositor that paints the first real composited frames. It absorbs the M0 scene ledger wholesale — the ledger's lifecycle (create/commit/destroy/client-gone) is now scene state, and its rig tests are now scene-state assertions.

## 2. The node model — born 3D-ready, implemented 2.5D

A [`SceneNode`](../crates/core/src/scene/node.rs) is one surface's canonical state. It carries, from day one:

| Field | Meaning | M1 status |
|-------|---------|-----------|
| `client` | owning `ClientKey` (attribution, bulk cleanup) | live |
| `committed` | has the surface committed at least once | live |
| `transform` | placement — a `Transform` | **Identity / Translate only** |
| `size` | `(width, height)` in pixels | live |
| `z` | stacking order (higher = on top) | live |
| `source` | pixel source — `Option<TextureSource>` | `Solid` + `Shm` (real, T3) |
| `opaque` | fully-opaque hint (blend + future occlusion) | live |
| `role` | what the surface *is* — and whether it may be displayed at all | `None` / `Toplevel` / `CoreOwned` (T5, §10) |

**The narrow line (Thesis 1 vs Thesis 3).** The `transform` slot and the `source` binding exist because a conventional window is the *degenerate case* of a 3D-textured object, not the only case (Thesis 1). But in M1 **only the axis-aligned integer-translation path is implemented, composited, or tested** (Thesis 3, "the cheap regime is sacred"):

```
enum Transform { Identity, Translate { dx: i32, dy: i32 } }   // + future: Affine2, Matrix(Mat4)
```

The enum is *extensible* (adding a real 3D transform is a new variant plus its composited path), but no transform math beyond integer translation is reachable today — there is no half-working rotation or perspective. Building 3D now would be scope creep; building a type that *forbids* 3D would be a Thesis-1 violation. The enum threads that needle: open vocabulary, narrow implementation. When a real transform lands it brings its own composited, golden-tested arm; until then the compositor's `match` is exhaustive over `Identity`/`Translate`.

A node is **visible** — and so contributes a snapshot node — only once it has a display-worthy `role`, a `source`, and a non-empty `size` (**§10**: the role gate is T5's mapping-semantics migration; before it, any committed surface composited). A freshly-created surface is live but silent until it takes a role and "attaches", exactly like a real client's surface before it becomes a window.

## 3. The texture-source seam (the Rayland seam)

`TextureSource` is the single point where the scene and renderer decide what a node is textured with, and it carries the load-bearing rule verbatim in its module doc:

> **Nothing in the scene or renderer may assume pixels are locally produced.**

A node does not know whether its texture came from a local shared-memory buffer, a local GPU dmabuf, or a Rayland replay service rendering on behalf of a board across the network (`VISION.md` Thesis 1; `CORE-BOUNDARY.md` C9). M1 ships two members:

```
enum TextureSource {
    Solid([u8; 4]),        // tests + C10 fallbacks; composited directly, no import
    Shm(Arc<PixelBuffer>), // T3: a wl_shm buffer decoded into a source-neutral pixel block
    // future (attach here, same rule governs all): Dmabuf(..), RaylandToken(..) [C9]
}
```

The `Shm` payload is a **source-neutral** `PixelBuffer { width, height, rgba }` — decoded RGBA8, tightly packed. The type name says "shm" (the per-origin variant convention: `Solid`/`Shm`/future `Dmabuf`/`RaylandToken`), but the *payload* carries no origin: the renderer blits a `PixelBuffer` and asks no questions, so the seam sentence stays literally true. The future `Dmabuf` and Rayland **token-buffer** sources attach at this enum — the whole of the M1 "Rayland interface obligation" the milestone plan pulls forward.

### 3.1 Copy-at-commit, immediate release (T3)

The `wl_shm` handling is `smithay::wayland::shm` only — **Smithay's renderer layer is never touched** (the seam check; verified clean, grep-verifiable). At commit, on the dispatch thread where the Wayland objects live (§7):

1. Take the just-committed `BufferAssignment` out of the surface's *current* state.
2. `NewBuffer` → `with_buffer_contents` copies + decodes the pixels into an owned `PixelBuffer` (`argb8888` → RGBA blend; `xrgb8888` → RGBA, alpha forced 255, opaque), the `wl_buffer` is **released immediately**, and the node's size + source are set on the scene. `Removed` (null attach) → the source is cleared (unmap). No buffer change → source unchanged.

**Rationale.** Correctness and client-compatibility first: immediate release lets single-buffer clients reuse their buffer at once, and an owned copy makes the buffer's lifetime irrelevant to the scene (destroy-after-commit is safe by construction). The copy is a memcpy on the **dispatch thread — not the frame path** (I-1). Zero-copy and damage-aware partial copies are later optimizations (**T4**). Format knowledge lives *only* in the copy path; the blit sees RGBA + the node's `opaque` flag. Snapshots share the `Arc<PixelBuffer>` by ref-count, so a full-copy snapshot never duplicates pixel data.

## 4. Thread ownership (CORE-BOUNDARY §7)

Ownership is absolute: each resource has exactly one owning thread; cross-thread communication is message passing; the scene graph crosses threads only as immutable snapshots.

```
   ┌────────────────────┐   ProtocolEvent          ┌──────────────────────────┐
   │  T-proto[0]        │   (SurfaceCreated,        │  T-scene                 │
   │  (ProtocolHost     │────Committed, Destroyed,──▶  (SceneThread)           │
   │   dispatch thread) │    ClientGone)            │  OWNS Scene = canonical  │
   │  owns Display,     │   SceneHandle::emit()     │  state (C4 / I-5)        │
   │  client sockets    │   one-way, async (I-3)    │                          │
   └────────────────────┘                           │  applies events +        │
                                                     │  visual-state setters    │
   test / (future T3/T5)──setters via SceneHandle───▶│                          │
   read via query()/wait_until() ◀───────────────────│                          │
                                                     └──────────┬───────────────┘
                                          Snapshot (immutable,   │ SceneHandle::snapshot()
                                          owned, back-to-front)  │ (the scene→render edge)
                                                                 ▼
                                                     ┌──────────────────────────┐
                                                     │  T-render                │
                                                     │  RenderLoop::tick():      │
                                                     │  pull Snapshot → composite│
                                                     │  → count frame (C5)      │
                                                     │  Compositor seam →        │
                                                     │  CpuCompositor (backend)  │
                                                     └──────────────────────────┘
```

- **T-scene** ([`SceneThread`](../crates/core/src/scene/thread.rs)) is the sole owner of `Scene`. Nothing else touches it; every reader and writer goes through a cloneable `SceneHandle` by message. This is the §7 ownership made real.
- **T-proto[0]** publishes lifecycle with `SceneHandle::emit` — fire-and-forget, never blocking on a reply (I-3). Only `Send` core tokens (`SurfaceId`, `ClientKey`) cross; never a borrowed `WlSurface`.
- **T-render** ([`RenderLoop`](../crates/core/src/render.rs)) pulls an immutable `Snapshot` and composites it. The snapshot is an *owned value*, so no lock is shared between the render (frame-path) thread and the scene thread (I-1).

**M1 simplification, stated honestly.** T-scene is a real dedicated thread. T-render is a **skeleton** driven by a **test-controlled `tick()`** — the caller decides when a frame is produced, so tests are deterministic and use no wall-clock (`harness_design.md` §4). The real frame scheduler (render-as-late-as-possible, tied to vblank / T-commit) arrives with the DRM backend in M2; until then "when to tick" belongs to the caller. This is a deliberate scaffold, not a hidden gap.

## 5. Snapshot semantics

`Scene::snapshot()` produces an immutable [`Snapshot`](../crates/core/src/scene/snapshot.rs): a **full owned copy** of the visible nodes, sorted **back-to-front** (ascending `z`, ties broken by `SurfaceId` for a deterministic total order). Sorting happens once, at snapshot time, off the hot path — the compositor iterates in draw order with no per-frame sort.

- **Isolation.** Because a snapshot is a self-contained owned value, once produced it is frozen: later mutations of canonical state cannot touch an in-flight frame. This is asserted directly (unit test in `scene::state`, cross-thread test in `scene::thread`, and end-to-end golden `scene_snapshot_isolation`).
- **§10.3 stays open.** Snapshot v1 is a `Vec` copy because for the handful of nodes M1 composites it is trivially cheap and obviously correct. Persistent structural sharing (copy-on-write of unchanged subtrees) is `CORE-BOUNDARY.md` §10.3 and remains an **open question** — deliberately not built (CLAUDE.md: no abstraction beyond what the task requires). It reopens with a benchmark, not a hunch.

## 6. The CPU compositor v1

[`CpuCompositor`](../crates/backend-headless/src/composite.rs) implements the core's [`Compositor`](../crates/core/src/render.rs) seam. It lives in the *backend* crate, alongside the `Frame` it renders into, so **`crates/core` depends on no backend** — the same discipline that keeps Smithay's renderer types out of the core. The core drives compositing through one trait method and never names `Frame`.

- Clears to a fixed colour, then paints each node back-to-front (painter's algorithm) — a solid rectangle clipped to the frame. Overlap resolves by draw order (later = on top).
- **Integer-only, deterministic, tolerance-0.** No floats, time, or randomness (`harness_design.md` §4). Opaque nodes overwrite; translucent nodes use an integer source-over blend (`out = (src·a + dst·(255−a) + 127)/255`), which reduces to an exact copy at `a = 255`.
- Clips nodes straddling any edge (signed offsets → intersect with the frame); a fully off-screen node draws nothing and never panics.
- No damage yet (T4): every tick clears and repaints in full. Damage will change *cost*, never *output*.

**Instrumentation.** [`FrameCounters`](../crates/core/src/render.rs) counts `frames_produced` and `nodes_composited` — the minimal counter *mechanism* T4 will extend with pixels-redrawn / region metrics. Golden tests assert on these totals.

## 7. Tests (what proves this works)

- **Scene state** (`scene::state`): migrated ledger lifecycle (create/commit/destroy/client-gone), visibility, snapshot sort order, snapshot copy-isolation.
- **Scene thread** (`scene::thread`): place+snapshot round-trip, query, cross-thread snapshot isolation, `wait_until` immediate/timeout.
- **Compositor** (`backend-headless::composite`): single node, back-to-front stacking, edge clipping, translucent blend, empty-frame clear, and (T3) opaque/translucent **pixel-block** blits.
- **Render loop** (`render`): tick pulls a snapshot and accumulates counters.
- **Protocol rig** (`harness/tests/protocol.rs`): the migrated ledger rig, now asserting on scene state through the host→scene edge, **plus the three T2 rig tests** (§8).
- **Goldens** (`harness/tests/scene_render.rs`, tolerance-0): `scene_two_overlap` vs `scene_two_overlap_restacked` (stacking visible; restack changes output), `scene_clipped` (edge clipping), `scene_snapshot_isolation` (in-flight isolation). The rig's willingness to reject a wrong frame was demonstrated once (a 1-px shift fails with `actual`/`golden`/`diff` artifacts).
- **Shm end-to-end** (`harness/tests/shm_render.rs`, tolerance-0): a scripted client draws a checkerboard-with-asymmetric-marker into a `wl_shm` buffer, attaches, and commits — `shm_xrgb` (opaque over solid), `shm_argb` (translucent blend visible), `shm_recommit` (single-buffer reuse: re-draw + re-commit the same buffer, second frame shown), plus `release`-after-commit and destroy-after-commit-safe assertions. Golden discrimination re-demonstrated for the new shm path (a one-row marker change is rejected).
- **Region algebra** (`scene::region`): intersect/translate/bounding/union/clip, empty-rect handling, and the coalesce-past-threshold collapse.
- **Damage** (`harness/tests/damage.rs`, §9.1): `incremental_equals_from_scratch` (the governing property, across an awkward structural + content sequence; verified to fail under sabotage), `small_damage_redraws_a_small_region` (proportionality via counters), `many_scattered_rects_coalesce_and_stay_correct`, `partial_copy_does_not_mutate_in_flight_snapshot` (CoW isolation, byte-checked).

## 8. The reverse path — frame callbacks, flush ownership, backpressure (T2)

M1 T1 opened one direction: client → scene. Wayland also needs the reverse —
`wl_surface.frame` callbacks fire when the compositor has *used* a commit, and
that decision is born on the render side. T2 builds that path, and installs the
backpressure policy at the same time, because the moment two threads feed each
other queues "what happens when one floods" stops being theoretical.

**Canonical for:** `crates/core/src/protocol.rs` (the dispatch-side machinery,
`FramePresenter`, `present`, `pump_display`) and the `RenderLoop` present call in
`crates/core/src/render.rs`.

### 8.1 One thread touches protocol objects (§7)

Every Wayland object — `WlSurface`, `WlCallback` — stays on the **dispatch
thread**. The render side never posts a protocol event; it only *enqueues a
notice* that a frame was presented. The dispatch thread, waking on that notice,
turns it into `wl_callback.done` sends. This is the simplest model that satisfies
CORE-BOUNDARY §7, and it keeps the door open to sharding without re-auditing send
sites. `DisplayHandle` is `Send + Sync`, but v1 deliberately does not exercise
cross-thread event posting.

```
   ┌──────────────────────────┐                       ┌────────────────────────┐
   │  T-render                │  FramePresenter        │  T-proto[0] (dispatch) │
   │  RenderLoop::tick(t):     │  ::present(t)          │  OWNS every WlSurface / │
   │  composite → then, if a   │──atomic store + ping──▶│  WlCallback             │
   │  presenter is attached,   │  (wait-free, I-1)      │                         │
   │  notify "presented @ t"   │  coalescing, 1 slot    │  ping wakes calloop →   │
   └──────────────────────────┘                       │  present(): drain each  │
                                                        │  surface's pending      │
                                                        │  frame_callbacks, send  │
                                                        │  wl_callback.done(t)    │
                                                        └───────────┬─────────────┘
                                                    ONE flush per loop│ iteration
                                                    (after all sources ran)
                                                                     ▼ to client sockets
```

### 8.2 The notice and the wakeup

The render→dispatch notice is the smallest thing that can cross the boundary: a
`u32` timestamp in an `AtomicU32` plus a `calloop` **ping**. `FramePresenter::present(t)`
does an atomic store then a ping — **wait-free from T-render** (a frame-path
thread): no lock shared with the dispatch thread, no synchronous reply, so I-1
holds. The ping wakes the dispatch loop's `PingSource` promptly — one wakeup, no
polling sleeps in the delivery path; callback latency *is* this wakeup.

### 8.3 Callback semantics v1 (documented as v1)

On each render tick, the dispatch thread fires **all** pending frame callbacks on
**every** surface, with the tick's timestamp. "Pending" means *committed*: a
`wl_surface.frame` request lands in the surface's double-buffered *pending* state
and is merged into *current* only on the commit that carries it — so a callback
never fires before its carrying commit (Smithay's `compositor` cache enforces
this; we only drain *current*). `wl_callback` is one-shot: draining sends `done`
and destroys it, so a later tick with no new frame request delivers nothing.

Firing is **not** gated on snapshot visibility. That is required, not a shortcut:
a client may commit a frame request on an unmapped, attach-less surface and must
still get its `done` (this is the milestone's reverse-direction proof test).
Real vsync pacing (`presentation-time`) and occlusion-aware throttling — only
firing for surfaces actually presented, which needs damage/visibility — are
**M2** (T4 supplies the visibility). The tick's timestamp is test-controlled and
deterministic here; monotonic-ms wall-clock in production.

### 8.4 Flush ownership — exactly one site

The dispatch loop flushes **once per iteration**, in the loop body, *after* every
source callback has run. Each source only *enqueues* bytes — client replies in
`pump_display`, `wl_callback.done` in `present` — and this is the sole place they
are pushed to the sockets. There is no other `flush_clients` in the core
(grep-verifiable). Concentrating the flush is what lets the render side stay a
pure enqueuer and keeps ordering obvious.

### 8.5 Backpressure policy (the I-10 fairness rider)

Both queues that now couple the two threads are bounded, and the policy is
kept deliberately simple (no QoS):

- **Render→dispatch notice: coalesced, single slot.** Many presents before the
  dispatch thread drains collapse to one wakeup (the ping edge) carrying the
  latest timestamp (the atomic). Bounded by construction.
- **Frame-callback state coalesces.** Per surface, pending callbacks are the list
  the protocol already bounds per commit; the per-tick notice collapses to "a
  frame happened" rather than a queue of per-event notices.
- **Per-client accounting at the `ProtocolHost` boundary.** A client's pending
  frame-callback backlog is capped at `MAX_PENDING_FRAME_CALLBACKS` (64 — ~1 s of
  unacknowledged frames at 60 Hz, orders of magnitude past honest need). This is
  the one queue a client can grow *without bound* on its own, since callbacks
  only drain on a tick it does not control. Over the cap, the dispatch loop
  **stops reading that client's socket** (it dispatches per client and skips the
  offender) until a tick drains its callbacks — never dropping messages, never
  stalling shard-mates. Unscheduling the socket also halts that client's scene
  emits, so the protocol→scene direction is bounded transitively; a pure
  scene-event flood is additionally bounded by the scene thread keeping up (a
  general slow-consumer bound is future work).

**A discovery worth recording (`#discovery`).** The `rs` wayland-backend reads a
ready client's socket *to `WouldBlock` in a single `dispatch_single_client`
call* — there is no per-request read cap. So the backlog is bounded by
`MAX_PENDING_FRAME_CALLBACKS` **plus one socket-read burst**, not a tight
per-event constant; the throttle prevents the *next* read, not the current one.
The client's own writes block once its kernel socket buffer fills — that is the
end-to-end backpressure.

**The v1 cost, and how M2 T0 paid it.** v1 could only *skip* a throttled client:
its socket stayed registered inside wayland-backend's single aggregate poll fd,
which therefore stayed permanently ready, and the dispatch loop turned
continuously with nothing to do — a busy-wait on the dispatch thread (never the
frame path, so I-1 was unaffected). **M2 T0 replaced the intake with per-client
readiness sources**, and throttling became literal: at `add_client` the client's
socket is `try_clone`d and *that* descriptor is registered with `calloop`, so a
throttled client's source can simply be **disabled**. No readiness, no wakeups,
no spin — the loop sleeps on its timeout. Measured: reproducing the old semantics
makes the loop turn **100 046 times in 300 ms** where the fixed build turns ~15
(`harness/tests/protocol.rs::a_sustained_flood_does_not_spin_the_dispatch_loop`).

**Hysteresis.** Throttle at `MAX_PENDING_FRAME_CALLBACKS` (64), resume below
`RESUME_PENDING_FRAME_CALLBACKS` (16 — a quarter). Re-arming at the same mark
would make a steady flooder toggle its registration on alternate ticks; the gap
means a merely-busy client resumes after one render tick while a real flooder
stays parked. The re-arm happens where the backlog is *drained* — in the
present/callback path — because a disabled client can never re-arm from its own
dispatch.

**Two alternatives, rejected, so they stay rejected:**

- *Edge-triggering the aggregate fd.* It stops the spin and starves shard-mates:
  the aggregate fd never goes quiet while a throttled client holds unread data, so
  no new edge arrives — including for other clients that become ready. Fairness
  is the whole point of the rider; trading it for CPU is the wrong direction.
- *Timer-based rate limiting.* Bounds how fast the loop spins without ending the
  spin. It would satisfy an "iterations stay bounded" assertion while leaving the
  promise unkept, which is worse than not fixing it: the test would then certify
  the wrong thing.

**Shard-readiness, as a side effect.** Per-client sources are the shape a future
shard takes ownership of — a shard is "these clients' sources plus their
`Display`". The restructure moves toward the spike's shard-count-agnostic
interface rather than bending it.

### 8.6 T2 tests (`harness/tests/protocol.rs`)

- **`scene_triggered_frame_callback_reaches_client`** — the milestone's
  reverse-direction proof: an attach-less commit carrying a frame request, then a
  render tick, delivers `done` with the tick's deterministic timestamp.
- **`frame_callback_lifecycle_conformance`** — no `done` before the carrying
  commit; fires once the commit is presented, with the tick's timestamp; one-shot
  (a later tick delivers nothing more).
- **`flooding_client_is_throttled_second_client_served_and_bounded`** — client A
  floods commits + frame requests; its backlog stays at the bound (its flood left
  unread), a well-behaved client B is served in one tick, and A is throttled, not
  disconnected. Verified to fail if the throttle is removed (backlog blows past
  the bound).

## 9. Damage tracking v1 (T4)

Until T4 every tick recomposited the whole frame. Now damage flows from the
protocol, through the scene, into the snapshot as an output-space region, and the
renderer recomputes only damaged pixels against a **retained** frame. This begins
earning the 2.5D damage-tracked regime (VISION Thesis 3; the I-9 seed).

**Canonical for:** [`region`](../crates/core/src/scene/region.rs) (the rect
algebra), the damage accumulation in [`state`](../crates/core/src/scene/state.rs),
[`SnapshotDamage`](../crates/core/src/scene/snapshot.rs), the retained-frame
[`CpuCompositor`](../crates/backend-headless/src/composite.rs), and the
partial-copy path in [`protocol`](../crates/core/src/protocol.rs).

### 9.1 The governing property

**Incremental must equal from-scratch.** For any sequence of commits and scene
changes, rendering incrementally (retained frame + damage) produces a frame
byte-identical to compositing from nothing. Damage may only change *cost*, never
*output*. This has its own test (`harness/tests/damage.rs::incremental_equals_from_scratch`)
and is the property everything else serves — it survives only if damage is
**conservative** (covers every changed pixel). It was checked to fail when a
damage class is dropped (sabotaging `set_z`'s damage → the restack step
mismatches).

### 9.2 Region algebra — conservative, bounded, no subtraction

[`Region`](../crates/core/src/scene/region.rs) is a small owned rect list with
`union` / `translate` / `intersect` / `clip`. Three rules:

- **Over-approximation is always legal; under-approximation is a bug class.**
  Every op rounds outward.
- **No subtraction.** Within a frame damage only grows; subtraction is where
  region code grows teeth and M1 needs none.
- **Bounded.** Past `MAX_DAMAGE_RECTS` (16 — a handful covers real content damage;
  the cap keeps a pathological many-small-rects client from bloating the
  bookkeeping) the region **coalesces to its bounding box**: over-approximate but
  O(1) to carry, and still correct. Coalescing is a cost knob, not a correctness
  one.

We deliberately do not import a region crate or Smithay's desktop-layer region
handling; this keeps the snapshot — and the backend — free of Smithay geometry
types.

### 9.3 Where damage comes from (scene side)

Frame damage is the union of, in output coordinates:

- **Client damage** — `wl_surface.damage` (surface coords) and
  `wl_surface.damage_buffer` (buffer coords), accumulated double-buffered and
  applied at commit. **Marked assumption:** with no buffer scale/transform in M1,
  surface and buffer coordinates coincide, so the two are merged at one site
  (`protocol::damage_to_rects`) that M2+ generalizes. Client rects are translated
  by the node's output offset and clipped to its extent.
- **Structural changes** — a node moved/resized damages old ∪ new extent; a
  restack damages the node's extent; map/unmap/destroy/client-gone damages the
  extent. These live in the scene setters (`set_geometry`, `set_z`, `set_source`,
  `clear_source`, `apply`), which damage as they mutate.
- **The full-output fallback** — the first frame, and any explicit `damage_full`
  ("don't know") case. Counted separately (`full_damage_frames`) so its frequency
  stays visible.

The content-vs-structural split lives in `Scene::attach_content`: if the extent
changed (map/move/resize) it damages old ∪ new extent; otherwise it is a pure
content update and it damages only the client's rects. This is what makes a small
commit redraw a small region. The scene accumulates this into `pending_damage`,
which `snapshot()` **drains** (hence `&mut self`) into
[`SnapshotDamage::{Full, Region}`].

### 9.4 Retained-frame rendering

The [`CpuCompositor`] keeps its previous frame. Per tick: `Full` → clear and
repaint everything; `Region` → for each rect, clear it and redraw the nodes
intersecting it, back-to-front, each clipped to the rect. Nodes wholly outside
damage cost nothing (they are skipped, not just clipped). Pixels outside every
damage rect keep their retained values. (Opaque-region occlusion culling is M5 —
not started.)

### 9.5 Damage-aware partial copy, with copy-on-write

The §3.1 pickup: at commit, only the damaged region of the buffer is copied into
the surface's pixel block (`protocol::build_pixel_block`). Because in-flight
snapshots share that block by `Arc`, patching goes through `Arc::make_mut` —
copy-on-write, so a snapshot's pixels are **never** mutated (its own test,
`partial_copy_does_not_mutate_in_flight_snapshot`, byte-checks this). The full
fresh-copy path is the fallback when there is no prior block, dimensions changed,
or damage covers everything. This extends snapshot isolation down to pixel level.

### 9.6 Counters (extending T1's skeleton)

[`FrameCounters`] gains `pixels_redrawn`, `damage_rects`, and `full_damage_frames`
(render side); `ProtocolHost::bytes_copied` reports buffer bytes copied at commit.
These are asserted in tests — a counter nobody asserts on drifts into a lie. The
proportionality test shows a small-damage commit on a large surface redraws
pixels within a generous bound of the damage area, not the whole frame.

## 10. Mapping semantics and roles (T5)

Until T5 the scene composited **any** surface that had committed pixels. That was
convenient for tests and **non-conformant**: Wayland is explicit that a
`wl_surface` without a role is never displayed. T5 ends it, and because this
changes what "in the scene" *means*, it is a migration with its own decision-log
entry, not a feature.

**Canonical for:** [`NodeRole`](../crates/core/src/scene/node.rs), the role gate in
`SceneNode::is_visible`, the role/title setters in
[`state`](../crates/core/src/scene/state.rs), and the xdg-shell half of
[`protocol`](../crates/core/src/protocol.rs).

### 10.1 The rule

A node contributes pixels only if **all** of these hold: it has a display-worthy
role, it has a source, and its size is non-empty.

```
enum NodeRole {
    None,                    // a bare wl_surface — never displayed
    Toplevel(ToplevelRole),  // xdg-shell toplevel: displayed once it has content
    CoreOwned,               // core-injected content (C10 fallbacks, harness fixtures)
}
```

`CoreOwned` is the deliberate exception and it is not a test hatch: the C10
fallback family (solid decorations, the "server crashed" surface) is content the
core places itself, travels through no protocol, and can carry no client role —
yet must be displayable, since it exists precisely for the case where everything
else is dead. The harness's scene-injected fixtures ride the same door.

**"Mapped" needs no flag.** A toplevel is mapped exactly when it has the role
*and* a committed source — which is what unmap already clears. Adding a separate
`mapped` bool would create a second source of truth that could disagree with the
first.

### 10.2 The lifecycle, strictly

1. **Initial commit, no buffer** → the compositor sends `configure` (0×0, no
   states: *you* choose your size — the core has no size policy to impose).
2. **`ack_configure`, then a buffer** → the buffer commit maps the toplevel: it
   takes its C10 placement (§10.4) and becomes visible scene content.
3. **A buffer before the ack** → protocol error, and nothing maps.
4. **Null attach, or `xdg_toplevel.destroy`** → unmap. The node loses its source
   (and on destroy its role, since the `wl_surface` may outlive its role object)
   with the structural damage T4's `clear_source`/`set_role` raise. After an
   unmap the initial commit/configure sequence must be run **again** before the
   surface may map — the protocol says so, and §10.3 explains why we rearm it
   ourselves.
5. **`xdg_wm_base` ping/pong** is implemented as a mechanism (`ProtocolHost::ping_clients`,
   `pongs_received`). There is no ping *scheduler* and no unresponsive-client
   handling: when to ping and what to do about silence is policy, and policy is
   S1's (M4).

Title and `app_id` are captured into canonical state (I-5) and **nothing branches
on them** in M1. They exist because the core is where the truth about a window
lives; the policy daemon and debug inspectors read them later.

### 10.3 Two honest notes about the Smithay seam

The seam held — `smithay::wayland::shell::xdg` reaches everything we need with no
renderer type, and the workspace grep stays clean — with two things worth
recording rather than discovering twice:

- **Smithay's unmap bookkeeping is inert for us.** Its toplevel commit hook
  detects unmap through surface state that only its *renderer helpers* populate,
  and we use none of them (we supply our own renderer — that is the whole point
  of the seam). So the hook silently does nothing, and the core re-arms the
  initial-configure dance itself on unmap. This is a consequence of the
  frontend/renderer split we chose, not a defect in it.
- **The buffer-before-ack error code differs from the spec's.** Smithay's
  `ensure_configured` posts `xdg_surface.not_constructed` (1) where the spec has a
  dedicated `unconfigured_buffer` (3). The `xdg_surface` object is never handed to
  the compositor through Smithay's public API, so posting our own is not
  reachable without forking that layer. We take Smithay's code, and the
  conformance test pins the code we actually send — pinning what *is* sent is
  what makes the test worth having.

`delegate_xdg_shell!` also drags in one piece of T6 machinery: its `xdg_popup`
dispatch is bounded on `SeatHandler`, so the core carries an **empty** `SeatState`
and a trait impl with no behaviour. No `wl_seat` global is created (that needs
`Seat::new`, which is T6), so nothing about input exists yet.

### 10.4 C10 placement — loudly temporary

Placement is a policy decision, and policy does not live in the core (§4 rule 4).
Until the reference policy daemon S1 arrives in **M4**, `CORE-BOUNDARY.md` C10's
"default window placement" fallback stands in, as a deterministic cascade of named
constants in `protocol.rs`:

| Constant | Value | Meaning |
|---|---|---|
| `CASCADE_ORIGIN_X` / `_Y` | 0, 0 | where the first toplevel's top-left corner goes |
| `CASCADE_STEP_X` / `_Y` | 32, 32 | offset added per subsequent toplevel |
| `CASCADE_WRAP` | 8 | steps before returning to the origin |

The placement is a pure function of the toplevel's creation index — no clock, no
randomness, no dependence on what else is on screen — because **goldens depend on
it**. The wrap exists because the core does not know the output size (outputs
arrive with the DRM backend in M2), so the cascade cannot clamp to a screen; it
must not walk off into an unbounded plane instead.

The origin being (0, 0) is also why the T3 shm goldens survived this migration
byte-for-byte: the first toplevel lands exactly where the old raw-commit path put
its content.

**What is placed is the *declared window*, not the surface (M2 T7 follow-up).**
`xdg_surface.set_window_geometry` is how a client with client-side decorations
says "my real window is this rectangle; what lies outside it is title bar, border
and shadow". foot declares `(0, -26, 696, 494)`: its title bar sits 26 px *above*
its surface origin, in subsurfaces. Placing the raw surface at the cascade slot
therefore pushed the first window's decorations off the top of the output — and
only the first, because every later slot has room above it. The placement now
subtracts the geometry's origin, so the declared rectangle lands where the policy
intends and the decoration overhang falls outside it, which is what every real
compositor does with it.

Found by Roland looking at the screen, not by a test: every rig client draws at
its own origin and declares no geometry, so nothing in the suite had the shape
that fails. There is a test now
(`a_window_is_placed_by_its_declared_geometry_not_its_surface_origin`), verified
to fail against the old placement.

### 10.5 What T5 explicitly did **not** change

- **Frame-callback semantics (§8.3) are untouched.** Every tick still fires all
  committed callbacks on every surface, *not* gated on visibility — so the
  reverse-direction proof test, which commits a frame request on an attach-less
  (and now roleless, and now definitively invisible) surface, keeps its meaning
  and passed unmodified. Occlusion/visibility-aware throttling remains M2.
- **Snapshot semantics are untouched.** The role gate lives in `is_visible`, which
  the snapshot builder already consulted.
- **Damage classes are untouched.** Map and unmap were already structural changes
  through `attach_content` / `clear_source`; role changes damage old ∪ new extent
  for the same reason.

### 10.6 T5 tests

- **`harness/tests/xdg.rs`** — the dance (one configure at 0×0, ack, map);
  `roleless_surface_is_never_displayed` (the migration's headline, from the wire);
  title/app_id into canonical state; ping→pong; the three protocol errors, each
  asserting the **specific code**, not merely a disconnect; unmap-on-destroy and
  unmap-on-null-attach with the damage read off the render counters, the latter
  proving the re-map needs a fresh configure; and the cascade's determinism.
- **`harness/tests/xdg_render.rs`** — `xdg_cascade`: two clients, two toplevels,
  one cascade step apart, golden-compared. Discrimination demonstrated: changing
  `CASCADE_STEP_Y` by one pixel is rejected with `actual`/`golden`/`diff`
  artifacts.
- **Migrated** to `map_toplevel`: the T3 shm goldens, T4's copy-on-write isolation
  test, and the null-attach rig test. Scene-injected (`place_solid`) tests are
  untouched by design.

## 11. Input, focus, and the nested backend (T6)

T5 gave the compositor windows. T6 gives it a user: a `wl_seat` with keyboard and
pointer, a focus policy, and a desktop window that shows the composited scene and
feeds real input into it.

**Canonical for:** [`input`](../crates/core/src/input.rs) (the funnel and the focus
routing table), the seat and input application in
[`protocol`](../crates/core/src/protocol.rs), and the whole of
[`crates/backend-winit`](../crates/backend-winit/).

### 11.1 One funnel, two producers

Every input source produces the same value — [`InputEvent`] — and hands it to
`ProtocolHost::input`, which is a **message into the dispatch thread**. The
dispatch thread, still the only thread that touches Wayland objects (§8.1,
unchanged), applies it to the Smithay seat handles.

```
   winit event loop ─┐
   (nested backend)  │   InputEvent          ┌────────────────────────┐
                     ├──(control channel)───▶│  T-proto[0] (dispatch) │
   test rig ─────────┘   non-blocking        │  seat handles:         │
   (ProtocolHost::input)                     │  keyboard.input(),     │
                                             │  pointer.motion/button │
                                             │  /axis + frame         │
                                             └────────────────────────┘
```

The funnel names **no Smithay type**: key codes are Linux evdev codes, buttons
are `BTN_*` codes, coordinates are output-space `f64`s. Backends depend on the
core and must not learn the protocol library's vocabulary — the same rule that
keeps `Frame` out of the core. It also means the rig injects events through
exactly the production path, so what CI exercises is what runs.

### 11.2 The §7 deviation — winit owns the main thread

`CORE-BOUNDARY.md` §7 gives input its own thread (T-input). **M1 does not have
one**, and this is the honest statement of why:

- **What the pure model says:** T-input owns input devices and the focus routing
  table; T-render owns presentation; they are separate threads.
- **What winit forces:** its event loop must run on the main thread and delivers
  *both* input intake and window presentation through that one loop. A nested
  backend cannot split them.
- **What we did:** in the nested backend, that single loop is "T-input" — it
  translates events into the funnel and drives the render tick.
- **Why it is bounded, not a redesign:** the *interface* a real T-input needs
  already exists and is already the only way in (the funnel); protocol objects
  are still touched only by the dispatch thread, so §7's actual ownership rule is
  intact; and it applies to the *development* backend alone.
- **What replaces it:** libinput on its own thread with the DRM backend, **M2**.
  It produces the same `InputEvent`s, and nothing downstream changes.

### 11.3 Focus, hit-testing, and the read-mostly replica

Two C10 fallback policies, both loudly temporary (S1 replaces them in M4):

- **Keyboard focus = the topmost mapped toplevel**, recomputed on every map and
  unmap. A re-focus that would change nothing sends nothing.
- **Pointer focus = the topmost mapped surface under the cursor**, with
  enter/leave on crossings and surface-local coordinates.

Both read a [`FocusMap`] — **a read-mostly replica**, owned by the dispatch
thread, of just enough scene geometry (rect + z per mapped surface) to answer
"who is on top?" and "who is under the cursor?".

**Why a replica rather than querying the scene.** Asking the scene thread on
every pointer motion is a synchronous cross-thread round-trip *on the input
path*, and it can queue behind the render thread's snapshot request — which is
exactly the coupling **I-2** forbids ("input delivery MUST NOT wait on
rendering"). §7 names this pattern for T-input directly: "focus routing table
(read-mostly replica of canonical focus)". The scene stays canonical (I-5); the
replica is derived and reconstructible, updated by the same dispatch-thread code
that publishes the map/unmap/resize to the scene, so it cannot drift without that
code being wrong about what it just said.

The replica's ordering (ascending `z`, ties by `SurfaceId`) is **the snapshot's
draw order**, deliberately. If the two disagreed, input would land on a window the
user cannot see.

The T5 rule extends here for free: only mapped toplevels are in the table, so a
roleless or unmapped surface receives no focus, no keys, and no pointer events —
what cannot be seen cannot be clicked.

**A coordinate trap, recorded.** Smithay's pointer API is given the focused
surface's **origin in global space** and derives the client's surface-local
coordinates itself. Passing the local position instead type-checks and produces
plausible-looking output (`(0, 0)` at every enter). `FocusMap::at` therefore
returns a [`Hit`] naming `origin` and `local` separately rather than one bare
tuple — the bug is easy to write once and impossible to see afterwards.

### 11.4 The nested backend (`crates/backend-winit`)

Raw winit + softbuffer, **not** `smithay::backend::winit` (which is welded to
Smithay's GLES renderer, the layer the threading-fit decision bypasses):

- **Presentation.** The retained `Frame` (tightly-packed RGBA8) is converted to
  softbuffer's `0x00RRGGBB` `u32` layout — one function, one channel-order
  comment, its own unit tests. Alpha is dropped: the window is opaque and the
  frame is already flattened against its clear colour.
- **Resize.** Window resize → `CpuCompositor::resize` (which forces the next
  composite to be full — its retained pixels describe the old geometry) **and**
  `Scene::damage_full`. The compositor holds that guarantee itself so a resize
  cannot tear through someone implementing only half the contract. The headless
  output keeps its fixed size, so goldens are untouched.
- **Keycodes.** winit `KeyCode` → evdev is a table in the backend (the core knows
  only evdev). It covers the standard typing set; **unmapped keys are dropped and
  counted**, never fatal — a media key must not take the compositor down, and a
  counter keeps the gap visible instead of silent. M2's libinput delivers evdev
  codes directly and makes the table unnecessary rather than bigger.
- **The socket.** `ProtocolHost::listen_auto` / `listen_at` bind a real Wayland
  socket and admit connections through the same seam as the rig's socketpairs.
  Living in the core (not the binary) is what makes it testable without a
  display — `harness/tests/socket.rs` drives a real client over a real socket.
- **`parhelion-dev`.** The thin binary: scene + host + render loop + window +
  socket, printing its `WAYLAND_DISPLAY`. All logic is in the libraries.

**The cursor.** Client `set_cursor` requests are accepted and ignored for
rendering: in nested mode the host desktop draws the cursor, and the hardware
cursor plane is M2 (C1). Accepting the request is not a protocol violation; the
client's chosen shape simply is not shown yet.

**Key repeat.** `repeat_info` is advertised (600 ms / 25 Hz). The compositor
generates no repeat events — since `wl_keyboard` v4 the protocol makes repeat the
client's job — so those two constants are the whole implementation, and that is
correct rather than missing.

### 11.5 T6 tests

- **`input` unit tests** (`core::input`): hit-testing (surface-local coordinates,
  half-open far edges), stacking resolution matching draw order, unmapped
  surfaces unroutable, empty-map behaviour.
- **`harness/tests/input.rs`**: seat capabilities + a keymap that really is xkb
  text; keys with evdev codes, monotonic serials, and modifiers ordered before
  the key they modify; focus following topmost across map/unmap (the enter/leave
  *sequence*, not just the end state); pointer crossing two cascaded windows with
  surface-local coordinates; a click reaching the window under the cursor and
  only after its enter; axis events; and roleless/unmapped surfaces receiving
  nothing.
- **`harness/tests/socket.rs`**: a real client over a real listening socket maps a
  window; the socket keeps accepting after the first client.
- **`backend-winit` unit tests**: the keycode table (values, no duplicates,
  unmapped counted not fatal) and the softbuffer conversion (channel order, alpha
  dropped, buffer reuse on shrink).

## 12. The output, the clipboard, and shutdown (T7/T7b)

### 12.1 `wl_output`

A real client asks the compositor about its screen *before* it draws: how big,
what scale, how fast. So `wl_output` is implemented properly rather than stubbed
— an advertised-but-hollow global is a lie to every future client, and this one
would be found out on the first frame.

One output, named `parhelion-0`, with:

- a **real mode**: the backend's size (the nested window's, restated on every
  resize through `ProtocolHost::set_output_size`) at `OUTPUT_REFRESH_MHZ`
  (60 Hz). The refresh is a claim M1 cannot yet keep — the render loop is
  externally ticked and has no vblank (§4) — but clients schedule against it, so a
  plausible number beats a zero. The real one comes from the connector in M2;
- **scale 1**, because we implement no scaling. Claiming otherwise would make
  every client draw at the wrong size;
- **zero physical size**, the honest answer for a nested window: there is no
  monitor, so there are no millimetres, and clients read 0 as "unknown" rather
  than computing a nonsense DPI;
- `wl_surface.enter` / `leave` as windows map and unmap — idempotent, so a
  redraw does not re-announce anything.

`xdg_output` rides alongside (Smithay derives it from the same state) and reports
logical geometry, which at scale 1 is the mode.

### 12.2 The clipboard and drag-and-drop (`wl_data_device_manager`)

Added when the acceptance run found that `foot` will not start without it. The
clipboard is not a shell feature — it is a service the display server owes every
client — so it is protocol machinery (C3) with canonical per-seat state, and it
lives in the core.

**The bytes never touch the compositor.** A copy publishes a *source*; a paste
asks the *offer* for a pipe; the two clients transfer through it directly. The
core brokers the introduction and gets out of the way, which is both the
protocol's design and the reason the clipboard costs the core nothing.

**Focus-gating is the v1 capability model.** Only the keyboard-focused client may
set the selection, and only the focused client is handed offers. That is the
protocol's own answer to "who may overwrite what the user copied"; it satisfies
**I-7**'s letter (a grant — "has focus" — checked in the core at request time);
and it is what real tooling is built around: `wl-copy` maps a toplevel purely to
obtain focus, sets the selection, and destroys the window again (observed with
`WAYLAND_DEBUG=1`). The call sits in `refocus_keyboard`, the one place focus
changes, rather than anywhere the word clipboard appears.

**A liveness gap, found by a test and fixed.** Smithay clears a dead selection
lazily — it checks whether the source still exists only when the selection is next
*sent*, which normally means on a focus change. So when the clipboard's owner dies
while focus does not change (a background client exits), the focused client is
left holding an offer backed by a corpse. `refresh_selection` closes that, and its
timing is load-bearing: it must run **after** the departing client's teardown, not
during it — the `destroyed` hook for a surface fires while that client's data
source is still alive, so a check there re-broadcasts a dying offer. Hence the
deferred flag, drained at the end of the dispatch pass.

**Drag-and-drop is refused, not half-built.** A client that starts a drag has its
source cancelled immediately — protocol-legal, and the client learns at once
rather than waiting on a drag that will never resolve. A real drag is a *pointer
grab*, and how grabs compose with the focus model (C10 now, S1 in M4, shaped and
3D windows later) is a design conversation of its own.

**Scheduled debt (I-7):** beyond the focus gate, access is ungated — no security
context, no per-client grant, no smaller grant set for remote clients. Ordinary
Wayland, correct for M1; C8's capability machinery (M4) is where it becomes a
policy question.

### 12.3 Subsurfaces — the tree (M2 T7)

The debt T7b measured wrongly, T0 measured correctly, and this section pays.
`wl_subcompositor` is now honoured: a subsurface is a scene node with a parent,
and its content composites.

**Canonical for:** the tree fields on [`SceneNode`](../crates/core/src/scene/node.rs)
(`parent`, `children`), the tree operations and flattening in
[`state`](../crates/core/src/scene/state.rs), and the effective-commit walk in
[`protocol`](../crates/core/src/protocol.rs).

#### The shape

A node gains a `parent` and an ordered `children` list. Two decisions carry the
rest:

- **A child's `transform` is parent-relative.** Absolute position is the
  accumulated offset down the chain, computed where it is needed. This is why
  moving a parent carries its whole subtree for free: no child's stored state
  changes at all.
- **The children list contains the parent's own id** as the marker for where the
  parent sits among its children. That is not cleverness borrowed from Smithay —
  it is the only representation that can express `place_below`, which puts a child
  *beneath* its parent. "Above the parent" and "below the parent" are positions in
  one list, not two lists.

Nesting is arbitrary; every walk is bounded by `MAX_SUBSURFACE_DEPTH` (16) as a
cycle guard, because these walks run on the scene thread and an unbounded
recursion there takes the compositor with it.

#### The mapping law, extended

> A node composites iff it has a display-worthy role, a source, a non-empty size —
> **and every ancestor does too.**

`SceneNode::is_visible` answers for the node alone; `Scene::is_mapped` walks the
chain. A subsurface of an unmapped window is not "hidden", it is *not mapped*, and
the T5 rule follows it down the tree: what cannot be seen cannot be clicked. foot's
pixel-less border subsurface is the case nature provided — role assigned, position
set, no buffer ever — and it composites nothing and takes no input.

#### Sync and desync

A **synchronized** subsurface (the protocol's default) caches its commits; they
become current at the nearest desynchronized ancestor's commit. A **desynchronized**
one applies its own. Smithay owns the caching, which means our job is *not acting
early*: `commit` returns immediately for a sync subsurface, and the effective
commit walks the whole subtree.

**Atomicity is the semantic heart**, and it is structural here: one effective
commit produces **one** `SurfaceUpdate` list and **one** scene message, so no
snapshot can land between a parent's new content and its children's. A client that
moves a window and repositions its decorations in a single commit is never
rendered half-moved. The golden pair
(`subsurface_sync_before_parent_commit` / `..._after_parent_commit`) pins both
frames, because "nothing appeared yet" is a claim about pixels.

#### Damage through the tree

Structural changes damage the **subtree's** old ∪ new rects: a parent's move takes
its children's pixels with it, and a restack changes what is visible inside pixels
nobody moved. One subtlety earned by measurement: a subsurface's position is
re-stated on every effective parent commit (that is how the protocol defers it),
so `set_subsurface_position` **must** be a no-op when the position is unchanged.
Without that check the acceptance run damaged 76% of the output per keystroke
instead of 0.6% — correct output, ruinous cost, and exactly the kind of thing the
counters exist to catch.

The equivalence oracle grew a tree sequence (map child, map grandchild, move
parent, move child, restack below parent, atomic batch, unmap parent) because
trees are where incremental rendering goes wrong.

#### Flattening, and why the renderer did not change

The snapshot is still a flat back-to-front list. Roots are ordered by `z` (ties by
`SurfaceId`); each root's tree is flattened in composition order with offsets
accumulated; the renderer consumes exactly what it always did and knows nothing
about trees. **The scene owns tree semantics; the renderer owns pixels.**

Input uses the same ordering, rebuilt on the dispatch side from Smithay's tree so
routing never waits on the scene (I-2, T6's discipline). Two rules ride along:
subsurfaces are hit-testable but never keyboard-focusable (the protocol gives them
pointer input only), and the routing table is sorted by `SurfaceId` — an
unsorted walk over a `HashMap` made "topmost" depend on hash iteration, which is a
bug this document would rather record than repeat.

### 12.4 Graceful shutdown

`parhelion-dev` binds a real socket, and the listening socket unlinks itself (and
its `.lock`) when dropped. A signal with no handler kills the process outright, so
`Drop` never runs — T6 shipped exactly that litter. The fix is the smallest thing
that works: the signal handler sets an atomic flag, the event loop notices it and
exits **through its normal path**, and every `Drop` runs on the way out. Nothing
is cleaned up *in* the handler. `SIGKILL` still leaves the files behind — that is
the kernel's contract, not a gap in ours, and wayland-server's lock protocol makes
a stale socket harmless on the next bind.

`--headless` runs the same compositor with no window, which is what makes the
binary's own plumbing testable where there is no display
(`harness/tests/dev_binary.rs` spawns it, signals it, and asserts the files are
gone).

### 12.5 T7/T7b tests

- **`harness/tests/acceptance.rs`** — the milestone's acceptance run (see §13).
- **`harness/tests/clipboard.rs`** — copy/paste between two rig clients with the
  bytes checked; the **focus gate asserted** with a third, unfocused client;
  replacement cancels the previous source; the owner's death clears the selection;
  a drag is cancelled rather than left hanging; and a round-trip between the real
  `wl-copy` and `wl-paste` programs.
- **`harness/tests/output.rs`**, **`conformance.rs`**, **`dev_binary.rs`** — the
  output's advertisement, the `wl_shm` rejection paths, the pinned global set, and
  the binary's socket/shutdown behaviour.

## 13. What later tasks add here

**M1 is complete.** The acceptance run passes as an automated test: `foot` runs
headlessly against the compositor, echoes typed input, and typing redraws 0.62% of
the output with every changed pixel inside the reported damage region — the
founding thesis, measured against software we did not write, and re-proved on
every CI push.

The named successors to this document's temporary parts, all **M2**: the real
T-input thread on libinput (§11.2), the vblank-tied frame scheduler that replaces
the test-controlled tick (§4), the cursor plane (§11.4), `presentation-time`
pacing and occlusion-gated frame callbacks (§8.3), and buffer scale/transform,
which un-merges the surface-vs-buffer coordinate site marked in §9.3. Placement
and focus policy leave for the policy daemon S1 in **M4** (§10.4, §11.3).

Anything requiring 3D transform math, opaque-region occlusion culling (M5), or
persistent snapshot sharing (§10.3) is out of scope until explicitly scheduled.
