# Parhelion — Scene Graph v1

> **Re-entrancy header.**
> **Status:** Draft v0.1 · **Date:** 2026-07-24 · **Kind:** subsystem design (the scene graph, render loop, and snapshot mechanism).
> **Upstream:** `VISION.md` Theses 1 & 3; `CORE-BOUNDARY.md` §3 (C4 scene graph, C5 render loop), §7 (threading), §10.3 (snapshot representation — open); `docs/plans/m1_tasks.md` T1.
> **Downstream:** T2 (frame callbacks / flush ownership / backpressure — landed, §8), T3 (wl_shm → `Shm` source), T4 (damage tracking), T5 (xdg-shell geometry), T6 (input/winit).
> **Canonical for:** `crates/core/src/scene/`, `crates/core/src/render.rs`, and the CPU compositor in `crates/backend-headless/src/composite.rs`.
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
| `source` | pixel source — `Option<TextureSource>` | `Solid` only (`Shm` declared) |
| `opaque` | fully-opaque hint (blend + future occlusion) | live |

**The narrow line (Thesis 1 vs Thesis 3).** The `transform` slot and the `source` binding exist because a conventional window is the *degenerate case* of a 3D-textured object, not the only case (Thesis 1). But in M1 **only the axis-aligned integer-translation path is implemented, composited, or tested** (Thesis 3, "the cheap regime is sacred"):

```
enum Transform { Identity, Translate { dx: i32, dy: i32 } }   // + future: Affine2, Matrix(Mat4)
```

The enum is *extensible* (adding a real 3D transform is a new variant plus its composited path), but no transform math beyond integer translation is reachable today — there is no half-working rotation or perspective. Building 3D now would be scope creep; building a type that *forbids* 3D would be a Thesis-1 violation. The enum threads that needle: open vocabulary, narrow implementation. When a real transform lands it brings its own composited, golden-tested arm; until then the compositor's `match` is exhaustive over `Identity`/`Translate`.

A node is **visible** — and so contributes a snapshot node — only once it has a `source` and a non-empty `size`. A freshly-created surface is live but silent until it "attaches", exactly like a real client's surface before its first buffer.

## 3. The texture-source seam (the Rayland seam)

`TextureSource` is the single point where the scene and renderer decide what a node is textured with, and it carries the load-bearing rule verbatim in its module doc:

> **Nothing in the scene or renderer may assume pixels are locally produced.**

A node does not know whether its texture came from a local shared-memory buffer, a local GPU dmabuf, or a Rayland replay service rendering on behalf of a board across the network (`VISION.md` Thesis 1; `CORE-BOUNDARY.md` C9). M1 ships exactly two members:

```
enum TextureSource {
    Solid([u8; 4]),   // tests + C10 fallbacks; composited directly, no import
    Shm,              // declared placeholder — T3 implements it; rejected in M1
    // future (attach here, same rule governs all): Dmabuf(..), RaylandToken(..) [C9]
}
```

`Shm` exists so the seam's shape is fixed now; no M1 code constructs it, and the compositor rejects it (`unimplemented!`). The future `Dmabuf` and Rayland **token-buffer** sources attach at this enum — that is the whole of the M1 "Rayland interface obligation" the milestone plan pulls forward: a seam, not an implementation.

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
- **Compositor** (`backend-headless::composite`): single node, back-to-front stacking, edge clipping, translucent blend, empty-frame clear.
- **Render loop** (`render`): tick pulls a snapshot and accumulates counters.
- **Protocol rig** (`harness/tests/protocol.rs`): the migrated ledger rig, now asserting on scene state through the host→scene edge, **plus the three T2 rig tests** (§8).
- **Goldens** (`harness/tests/scene_render.rs`, tolerance-0): `scene_two_overlap` vs `scene_two_overlap_restacked` (stacking visible; restack changes output), `scene_clipped` (edge clipping), `scene_snapshot_isolation` (in-flight isolation). The rig's willingness to reject a wrong frame was demonstrated once (a 1-px shift fails with `actual`/`golden`/`diff` artifacts).

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
end-to-end backpressure. **v1 cost, chosen deliberately (keep it simple):** while
a throttled client has unread data the level-triggered `Display` source stays
ready, so the dispatch loop spins to keep serving others during an active flood.
This is the dispatch thread, *not* the frame path (I-1 is unaffected), and the
tighter fix (per-client readiness / edge management) is M2.

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

## 9. What later tasks add here

| Task | Adds to the scene graph |
|------|-------------------------|
| **T3** | `wl_shm` buffers → the `Shm` texture source becomes real (CPU copy); buffer attach/commit/release lifecycle drives node geometry/source. |
| **T4** | Per-surface damage accumulation; region algebra (surface→scene→output); partial redraws honouring damage; the counter mechanism grows pixels-redrawn / region counts. |
| **T5** | xdg-shell: roles, configure/ack, map/unmap, title/app_id into scene state; default placement from C10. |
| **T6** | Seat/input; focus = topmost mapped toplevel (temporary C10 policy); the winit nested backend presenting these frames. |

Anything requiring 3D transform math, `presentation-time`, or persistent snapshot sharing (§10.3) is out of scope until explicitly scheduled.
