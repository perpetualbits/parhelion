# Parhelion — Decision Log

**Status:** append-only living document.
**Purpose:** record load-bearing decisions with date, source, and
one-line summary. Reasoning stays in subsystem documents; this log
tells you what was decided and where to read more.

New chats: read this immediately after the project index.
(Convention adopted from ENO — `eno_decision_log.md`. The earlier
ADR-style `decisions/0002-*.md` is superseded in *format* by this log;
its content is carried as the 2026-07-24 procedural-content entry
below, reasoning now living in the dialect spec and VISION.md.)

---

## 2026-07-23 — Founding documents

### Compositor is a Wayland-speaking 3D scene-graph engine with microkernel discipline

- **Source:** Parhelion design chat #1.
- **Affects:** `VISION.md` (whole), `CORE-BOUNDARY.md` (whole).
- **Reasoning:** the three goals (Rayland reference S-side,
  non-rectangular windows, 3D desktop objects) are one architecture;
  GNOME's in-process extension model is the anti-pattern.

### Process boundaries follow fault/timing/privilege, not memory

- **Source:** Parhelion design chat #1.
- **Affects:** `CORE-BOUNDARY.md` §4 (placement algorithm), §5 (I-1..I-4).
- **Reasoning:** Rust covers memory safety in-process; IPC buys
  isolation only where blocking, crashing, third-party code, or
  hostile input is possible.

### Two rendering regimes with mandatory collapse

- **Source:** Parhelion design chat #1.
- **Affects:** `VISION.md` Thesis 3, `CORE-BOUNDARY.md` C6, I-9.
- **Reasoning:** damage tracking and game-style full-frame rendering
  solve different problems; the desktop needs both plus a state
  machine that returns to the cheap regime within ≤2 frames.

### Canonical state in core; servers are restartable views

- **Source:** Parhelion design chat #1.
- **Affects:** `CORE-BOUNDARY.md` I-5, §8.
- **Reasoning:** crash-only beats crash-resistant; every control-plane
  protocol carries full resync.

## 2026-07-24 — Procedural content and vocabularies

### Application procedural content travels as shaders + parameters; fixed vocabularies only where compositor-owned

- **Source:** Parhelion design chat #2.
- **Affects:** `parhelion_desktop_dialect.md` §2 reserved
  (`desktop.decor.*`, `desktop.shape.*`), Rayland asset-cache priority.
- **Reasoning:** curated cross-system algorithm libraries for
  *application* content lose to vocabulary lock-in (NeWS, Display
  PostScript precedent); SPIR-V + content-hash cache is the open
  vocabulary. Small closed vocabularies are correct only where the
  compositor owns both ends (shapes, animation IR, decorations).

### Shape is declared, not extracted

- **Source:** Parhelion design chat #2.
- **Affects:** future shape-extension spec; `desktop.shape.*` reserve.
- **Reasoning:** clients know their outline; declared paths give
  analytic occlusion/damage/input regions. Alpha-contour extraction is
  compatibility fallback only.

## 2026-07-24 — Control plane adopts SPINE (full adoption)

### Parhelion's control plane is a SPINE dialect; C7 is a dialect interpreter

- **Source:** Parhelion design chat #3, after review of ENO
  `spine_core_v0_4_design.md`, `nerve_runtime_model.md`,
  `spine_graphics_dialect.md`, `spine_dialect_template.md`.
- **Affects:** `parhelion_desktop_dialect.md` (new, canonical),
  `CORE-BOUNDARY.md` §9 and C7 (to be amended to reference the
  dialect spec), I-6/I-12 (satisfied by construction).
- **Reasoning:** SPINE's seven ops + dialect contract subsume the
  planned bespoke animation IR with stronger structure; opacity rule
  and shape system double as capability boundary and admission
  control.

### Decoupling: vendored pinned core spec, sibling runtimes, deliberate imports

- **Source:** Parhelion design chat #3; requirement stated by Roland
  (ENO must evolve freely).
- **Affects:** `parhelion_desktop_dialect.md` §0.1,
  `third_party/spine/` (to be created).
- **Reasoning:** shared language, separate runtimes. ENO is upstream;
  core changes enter Parhelion only via logged import. NERVE and C7
  share no code in v0.1.

### Event-anchored time, interruption/retraction, submit-time expansion

- **Source:** Parhelion design chat #3.
- **Affects:** `parhelion_desktop_dialect.md` §6, §7.
- **Reasoning:** demos are non-interactive, desktops are; anchors,
  retargetable springs, HOLD-on-retract, and per-property
  last-writer-wins are dialect-level extensions — SPINE core
  untouched (guitar-strum test: gestures with internal time offsets
  are compounds; core ops suffice).

### Signal-shape LNK formalized as typed lock-free channel

- **Source:** Parhelion design chat #3; answers ENO
  `nerve_runtime_model.md` §9.7 from the compositor side.
- **Affects:** `parhelion_desktop_dialect.md` §3; ENO mirror entry
  suggested (Roland's call, in ENO's log, not assumed here).
- **Reasoning:** signal edges cross thread/process boundaries by
  construction in Parhelion, need admission cost accounting and
  defined transport (SPSC latest-value vs queued events) — three
  consumers make it a language-level distinction, not an
  implementation detail.

### Wire form: JSON over the control socket; SPINE binary not adopted (v0.1)

- **Source:** Parhelion design chat #3; "do what is most practical"
  (Roland).
- **Affects:** `parhelion_desktop_dialect.md` §10.
- **Reasoning:** fragments are hundreds of bytes on a local socket;
  64k discipline is ENO's constraint. Revisit on profiling evidence or
  ENO binary-toolchain stability; requires entries in both logs.

## 2026-07-24 — Project name confirmed

### The project's official name is Parhelion (no longer a working title)

- **Source:** Roland, directly, during the M0 scaffolding session.
- **Affects:** `VISION.md` §7 and its re-entrancy header (both still call
  "Parhelion" a placeholder/working name — now stale; flagged for Roland
  to amend, not edited here); the Pending item below (resolved).
- **Reasoning:** the *sundog* metaphor and the Sun Ray → Rayland →
  Parhelion lineage settle it; the alternatives (Sundog, Analemma,
  Firmament, Penumbra) are dropped. The repo, crate names (`parhelion-*`),
  and docs already use it.

## 2026-07-24 — Smithay threading fit (M0 task 2)

### Smithay is consumed as the protocol frontend (+ later hardware backends); its renderer and desktop/space layers are bypassed

- **Source:** M0 task 2 investigation spike; report `docs/smithay_threading_spike.md`; runnable evidence `tools/spikes/smithay-threading/`.
- **Affects:** `crates/core/` protocol layer; `CORE-BOUNDARY.md` §7 (satisfied, not amended) and open question §10.4 (resolved); M0 task 3 (headless backend + protocol harness).
- **Decision:** Consume `smithay::wayland::*` (protocol handlers, delegate macros) on `wayland-server` 0.31 as the protocol frontend, and Smithay's `backend::*` (DRM/session/libinput/udev/egl/allocator, winit) at M1/M2. Bypass Smithay's `renderer` and `desktop` layers — Parhelion supplies its own 3D-native renderer, scene graph, and regime machine. Wrap `input` and dmabuf/syncobj.
- **Reasoning:** `Display<State>` is unconditionally `Send + Sync` (State is borrowed at dispatch, not owned), so protocol state can live on a dispatch thread and publish scene changes by message — the §7 T-proto[n] → scene edge — proven to compile and run. Smithay's API churn is concentrated in the renderer/DRM layers (the bypassed/wrapped ones); the depended-upon frontend is its most stable layer.

### Protocol dispatch runs at shards = 1 behind a shard-count-agnostic ProtocolHost interface

- **Source:** same spike.
- **Affects:** `crates/core/` protocol layer; M1 thread skeleton.
- **Decision:** Dispatch runs single-threaded (shards = 1) now. The core's protocol layer is structured as a `ProtocolHost` that assigns each accepted client to a shard at accept time (`ListeningSocket::accept` → `DisplayHandle::insert_client`), so growing to N `Display`-per-thread shards is an implementation change, not an architectural one. Requirements: canonical state reachable from protocol threads only by message; only `Send` tokens (`ObjectId`/core `SurfaceId`) cross to the scene; globals/capabilities advertised identically per-`Display`.
- **Reasoning:** CORE-BOUNDARY §7 specifies ownership, not a mandatory shard count. Protocol dispatch is not the frame path, so I-1/I-2 are unaffected by a single dispatch thread; escalate 1→N only on measured contention. No §7 conflict was found.

### Smithay is pinned exactly (=0.7.0) with a committed lockfile; upgrades are deliberate

- **Source:** same spike; cosmic-comp's git-pin pain as counter-example.
- **Affects:** `crates/core/Cargo.toml` (when the protocol layer lands); dependency-update policy.
- **Decision:** Pin `smithay = "=0.7.0"` (exact) + commit `Cargo.lock`; upgrades are batched, deliberate, and land behind the `ProtocolHost`/renderer-wrapper seams. Mirrors the SPINE vendored-pinned-deliberate-import discipline.
- **Reasoning:** Smithay now ships on crates.io (0.4.0+, 2025); exact-pin + lockfile avoids the `wayland-backend` resolution conflicts that dominate downstream (cosmic-comp) breakage reports. Pins recorded in the spike's `Cargo.lock`: smithay 0.7.0, wayland-server 0.31.14, wayland-backend 0.3.16, calloop 0.14.4.

## 2026-07-24 — Scene graph v1 (M1 T1)

### Scene state is born 3D-ready but implemented 2.5D via an extensible `Transform` enum

- **Source:** M1 T1 (prompt 04); `docs/scene_graph_v1.md` §2.
- **Affects:** `crates/core/src/scene/node.rs` (`Transform`, `TextureSource`, `SceneNode`); VISION Theses 1 & 3.
- **Decision:** A scene node carries a `transform` slot and a `source` binding from day one, but only `Transform::{Identity, Translate}` (integer, axis-aligned) is constructible/composited/tested. `Transform` is an enum so real transforms (affine, 4×4) are future variants, each landing with its own composited path; the compositor's `match` is exhaustive over what exists, so no transform math beyond translation is reachable.
- **Reasoning:** Building 3D now is scope creep; a type that *forbids* 3D is a Thesis-1 violation. An extensible enum keeps the vocabulary open and the implementation narrow — the exact line Thesis 3 draws.

### Texture sources are an extensible binding; the seam rule is "nothing may assume pixels are locally produced"

- **Source:** M1 T1; `docs/scene_graph_v1.md` §3.
- **Affects:** `crates/core/src/scene/node.rs` (`TextureSource`); `CORE-BOUNDARY.md` C9; Rayland hosting obligation.
- **Decision:** `TextureSource` has exactly two members in M1: `Solid([u8;4])` (tests + C10 fallbacks) and a declared `Shm` placeholder (T3 implements; rejected until then). `Dmabuf` and the Rayland token-buffer source (C9) attach at this enum later. The module doc carries the seam sentence verbatim.
- **Reasoning:** This binding is the whole M1 "Rayland interface obligation" — a fixed seam, not an implementation. Fixing its shape now means no scene or renderer code ever assumes locally-produced pixels.

### The render target lives behind a core-defined `Compositor` seam; the core depends on no backend

- **Source:** M1 T1; `docs/scene_graph_v1.md` §6; `crates/core/src/render.rs`.
- **Affects:** `crates/core` (the `Compositor` trait, `RenderLoop`), `crates/backend-headless` (implements it, gains a `parhelion-core` dependency), the crate dependency graph.
- **Decision:** `crates/core` defines a one-method `Compositor` trait (`composite(&Snapshot) -> usize`) and drives it from `RenderLoop::tick`, never naming a concrete frame type. The CPU compositor and the `Frame` it paints live in `backend-headless`, which depends on `core` (never the reverse). M2's DRM backend implements the same trait.
- **Reasoning:** Keeps `Frame` and every backend type out of the core — the same discipline that keeps Smithay's renderer types out (decision "Smithay threading fit"). The one small trait is the C5↔C1/backend seam, not a premature abstraction; without it, `core` would have to depend on a backend crate.

### Scene owned by a dedicated thread; T-render is a test-ticked skeleton in M1

- **Source:** M1 T1; `docs/scene_graph_v1.md` §4; `CORE-BOUNDARY.md` §7.
- **Affects:** `crates/core/src/scene/thread.rs` (`SceneThread`/`SceneHandle`), `crates/core/src/render.rs` (`RenderLoop`), `crates/core/src/protocol.rs` (now publishes to the scene).
- **Decision:** `Scene` (canonical state) is owned by one dedicated `SceneThread`; all access is by message through a cloneable `SceneHandle` (emit / mutate / snapshot / query / wait_until). `ProtocolHost` publishes lifecycle into it. Snapshots cross to T-render as immutable owned values. T-render is a **skeleton** driven by an externally-controlled `tick()` (deterministic, no wall-clock); the real vblank-tied frame scheduler is M2 (DRM backend).
- **Reasoning:** The load-bearing §7 property is single-owner canonical state with snapshots crossing threads — that is real now. Standing up a full vblank-driven render thread before there is real hardware or a frame deadline would be dishonest RT; the test-ticked skeleton is the correct scaffold and is documented as such. §10.3 (persistent snapshot sharing) stays open — snapshot v1 is a full copy.

### The M0 scene ledger is absorbed into the scene graph and deleted

- **Source:** M1 T1; `docs/scene_graph_v1.md` §1.
- **Affects:** `crates/core/src/ledger.rs` (deleted); `crates/core/src/scene/state.rs` (`ProtocolEvent` succeeds `LedgerMsg`); `crates/harness/tests/protocol.rs` (asserts on scene state).
- **Decision:** The M0 ledger (`Ledger`, `LedgerMsg`, `ProtocolHost::{sync,ledger,wait_until}`) is gone. Its lifecycle behaviour is now `Scene::apply(ProtocolEvent)`; its rig tests migrated to scene-state assertions via `SceneHandle::query`. `ProtocolHost::new` now takes a `SceneHandle`.
- **Reasoning:** The ledger was explicitly a M0 stand-in "the M1 scene graph must not fight" — M1 replaces it wholesale, as designed. One canonical receiver of the protocol→scene edge, not two.

## 2026-07-24 — Reverse path: frame callbacks, flush ownership, backpressure (M1 T2)

### The render side only enqueues a notice; the dispatch thread owns all protocol-object interaction and the single flush

- **Source:** M1 T2 (prompt 05); `docs/scene_graph_v1.md` §8.
- **Affects:** `crates/core/src/protocol.rs` (`FramePresenter`, `present`, single `flush_clients` site, `PingSource`), `crates/core/src/render.rs` (`RenderLoop::tick(time_ms)`, optional presenter); CORE-BOUNDARY §7, I-1.
- **Decision:** T-render never touches a Wayland object. It calls `FramePresenter::present(t)` — a wait-free atomic store + `calloop` ping — and the dispatch thread turns that into `wl_surface.frame` → `wl_callback.done(t)` sends. Flushing is one site only: the loop body flushes once per iteration after all sources ran; every source callback only enqueues. Grep-verifiable (one `flush_clients`, no `DisplayHandle` use outside `protocol.rs`).
- **Reasoning:** One thread owning protocol state (§7) is the simplest model and keeps the door open to sharding without re-auditing send sites. The present notice is non-blocking from the frame path (I-1). Concentrating the flush makes ordering obvious and keeps the render side a pure enqueuer.

### Callback semantics v1: every tick fires all pending callbacks, not gated on visibility

- **Source:** M1 T2; `docs/scene_graph_v1.md` §8.3.
- **Affects:** `crates/core/src/protocol.rs` (`present`); snapshot semantics **unchanged** (deliberately out of scope).
- **Decision:** On each render tick, all *committed* (current, not pending) frame callbacks on every surface fire with the tick's timestamp; `wl_callback` is one-shot. Firing is **not** gated on snapshot visibility — an attach-less/unmapped surface's callback still fires. Real vsync pacing (`presentation-time`) and occlusion-aware throttling are M2 (need damage/visibility from T4).
- **Reasoning:** The reverse-direction proof test requires an invisible surface's callback to fire, so visibility gating would be wrong in v1. Not consulting the snapshot for callback routing means no change to snapshot semantics.

### Backpressure: per-client pending-callback cap enforced by leaving the socket unread

- **Source:** M1 T2; `docs/scene_graph_v1.md` §8.5; the I-10 fairness rider from the spike review; Roland's "keep it simple, note it" ruling on the flood-time CPU cost.
- **Affects:** `crates/core/src/protocol.rs` (`MAX_PENDING_FRAME_CALLBACKS = 64`, per-client `pump_display` throttle via `dispatch_single_client`); invariant I-10.
- **Decision:** A client whose pending frame-callback backlog reaches `MAX_PENDING_FRAME_CALLBACKS` has its socket left unread (the loop dispatches per client and skips it) until a tick drains it — never dropped, never a shard-mate stall. This bounds both the callback queue and, transitively, that client's scene emits. The render→dispatch notice is coalesced to a single slot. **Discovery (recorded):** the `rs` backend reads a ready client to `WouldBlock` in one call, so the bound is `cap + one socket-read burst`, not a tight per-event constant; and a throttled client with unread data keeps the level-triggered source ready, so the dispatch thread (not the frame path) spins during an active flood. The tighter fix is M2 (chosen deliberately: keep it simple).
- **Reasoning:** The pending-callback backlog is the one queue a client can grow without bound on its own (callbacks drain only on a tick it does not control), so it is the correct throttle signal and the one that makes the flooding test fail if the bound is removed (verified). Bounding by socket-unschedule needs no blocking of the dispatch thread (I-3 spirit preserved: the emit edge stays fire-and-forget).

## 2026-07-24 — wl_shm buffers, copy-at-commit, and the seam check (M1 T3)

### Seam verdict: shm reaches buffer bytes through `smithay::wayland::shm` alone — clean

- **Source:** M1 T3 (prompt 06), the seam check flagged since the spike review; `docs/scene_graph_v1.md` §3.1.
- **Affects:** `crates/core/src/protocol.rs` (shm wiring), the consume/bypass layer table (decision "Smithay threading fit" entry 2 — confirmed, not amended).
- **Verdict:** **Clean.** `smithay::wayland::shm::{ShmState, delegate_shm!, with_buffer_contents}` reaches the buffer bytes (`*const u8`, `len`, `BufferData{width,height,stride,format}`) with **no `smithay::backend::renderer` type**. Grep-verifiable: `smithay::backend::renderer` appears nowhere in the workspace. Builds under `default-features = false, features = ["wayland_frontend"]` (the shm module and `backend::allocator::format` it uses internally are not renderer-gated). The spike's layer-table prediction held.
- **Reasoning:** This is exactly the frontend/renderer split the "Smithay threading fit" decision bought: consume the protocol frontend, supply our own renderer. Had the seam required a renderer trait to reach pixels, that would have been design news; it did not.

### Copy-at-commit into a source-neutral pixel block, released immediately

- **Source:** M1 T3; `docs/scene_graph_v1.md` §3, §3.1.
- **Affects:** `crates/core/src/scene/node.rs` (`PixelBuffer`, `TextureSource::Shm(Arc<PixelBuffer>)`), `crates/core/src/protocol.rs` (`commit` buffer path, `copy_shm_to_pixels`), `crates/backend-headless/src/composite.rs` (pixel-block blit); C9.
- **Decision:** At commit, on the dispatch thread, the attached `wl_shm` buffer is copied and decoded into an owned `PixelBuffer` (RGBA8, tightly packed), the `wl_buffer` released immediately, and the node's size + source set on the scene. `argb8888` → blend; `xrgb8888` → opaque (alpha forced 255). Format knowledge lives only in the copy path; the compositor blits a source-neutral `PixelBuffer` over the existing `opaque`/`source_over` machinery. `TextureSource`/`SnapshotNode`/`SceneNode` lose `Copy` (they may hold an `Arc`); snapshots share pixels by ref-count.
- **Reasoning:** Correctness and client-compatibility first (immediate release runs single-buffer clients; an owned copy makes destroy-after-commit safe by construction). The copy is a memcpy on the dispatch thread, **not** the frame path (I-1). Zero-copy / damage-aware partial copy is T4. Decoding to neutral RGBA at the copy keeps the renderer source-agnostic — the seam sentence stays literally true.

## 2026-07-25 — Damage tracking v1 (M1 T4)

### Damage is conservative, bounded, and subtraction-free; coalesces to a bounding box past a threshold

- **Source:** M1 T4 (prompt 07); `docs/scene_graph_v1.md` §9.
- **Affects:** `crates/core/src/scene/region.rs` (`Rect`, `Region`, `MAX_DAMAGE_RECTS = 16`); the damage accumulation in `scene/state.rs`; invariant I-9 (this seeds the 2.5D damage-tracked regime).
- **Decision:** The damage region is an owned rect list with over-approximating ops and **no subtraction**. Past `MAX_DAMAGE_RECTS` rects it collapses to its bounding box (over-approximate, O(1) to carry). This is a cost knob, not a correctness one — any value ≥ 1 is sound. We do not import a region crate or Smithay's desktop-layer region handling (keeps the snapshot/backend free of Smithay geometry types); `smithay::utils` geometry is only touched at the protocol boundary.
- **Reasoning:** The governing property — incremental rendering byte-identical to from-scratch — survives only if damage covers every changed pixel. Over-approximation is always safe; under-approximation is a bug class. Subtraction is where region code grows teeth (rect-splitting, fragment explosions) and M1 needs none. A bounded region stops a many-small-rects client from making the bookkeeping itself unbounded.

### Content-vs-structural damage split; retained-frame rendering; partial copy with copy-on-write

- **Source:** M1 T4; `docs/scene_graph_v1.md` §9.3–§9.5.
- **Affects:** `scene/state.rs` (`attach_content`, damage in setters, `snapshot(&mut)`), `scene/snapshot.rs` (`SnapshotDamage`), `backend-headless/src/composite.rs` (retained frame), `protocol.rs` (`build_pixel_block`); I-1 (snapshot stays an owned lock-free copy; the copy is off the frame path).
- **Decision:** The scene accumulates output-space damage as it mutates and drains it at snapshot. A content commit damages only the client's rects when the extent is unchanged, but old ∪ new extent on a structural change (map/move/resize). The compositor retains its frame and recomputes only within damage. At commit, only the damaged buffer region is copied into the surface's pixel block, via `Arc::make_mut` (copy-on-write) so an in-flight snapshot's shared pixels are never mutated; full copy is the fallback (no prior block / dims changed / covers-all). `Snapshot::snapshot` becomes `&mut self` to drain damage. `TextureSource` etc. lost `Copy` in T3, so snapshots already share pixels by `Arc`.
- **Reasoning:** Damage changes cost, never output. The split is what makes a small commit cheap while keeping structural changes correct. CoW extends snapshot isolation to the pixel level without ever copying more than needed. Counters (`pixels_redrawn`, `damage_rects`, `full_damage_frames`, `bytes_copied`) make the savings measurable and are asserted — an unasserted counter drifts into a lie.

## 2026-07-25 — xdg-shell minimal, and the mapping-semantics migration (M1 T5)

### Only mapped toplevels (and core-injected content) are displayed; a roleless surface never composites

- **Source:** M1 T5 (prompt 08); `docs/scene_graph_v1.md` §10.
- **Affects:** `crates/core/src/scene/node.rs` (`NodeRole`, `ToplevelRole`,
  `SceneNode::role`, `is_visible`), `scene/state.rs` (`set_role`/`set_title`/
  `set_app_id`), `scene/thread.rs` (`place_solid`), `crates/core/src/protocol.rs`
  (the xdg lifecycle), every client-driven test in the harness; invariant I-5
  (role and title/app_id are now canonical state).
- **Decision:** A scene node is visible only if it has a **display-worthy role**,
  a source, and a non-empty size. `NodeRole` has three members: `None` (a bare
  `wl_surface` — never displayed, per Wayland), `Toplevel(ToplevelRole)` (xdg-shell;
  displayed once it has committed content), and `CoreOwned` (content the core
  places itself — the C10 fallback family and the harness's scene-injected
  fixtures — which travels through no protocol and so can carry no client role).
  "Mapped" gets **no separate flag**: role + source is the definition, and unmap
  already clears the source. Client-driven tests migrate to a harness helper that
  performs the full dance; scene-injected tests are untouched.
- **Reasoning:** Compositing any committed surface was convenient and
  non-conformant, and it changes what "in the scene" means — which is why this is
  a logged migration rather than a feature. A separate `mapped` bool would be a
  second source of truth able to disagree with the first. `CoreOwned` is not a
  test hatch: C10 content must be displayable precisely in the case where every
  server is dead and no client role exists to grant it.

### Default placement is a deterministic C10 cascade until S1 (M4)

- **Source:** M1 T5; `docs/scene_graph_v1.md` §10.4; `CORE-BOUNDARY.md` C10, §4 rule 4.
- **Affects:** `crates/core/src/protocol.rs` (`CASCADE_ORIGIN_X/Y`,
  `CASCADE_STEP_X/Y = 32`, `CASCADE_WRAP = 8`), the `xdg_cascade` golden.
- **Decision:** A toplevel's placement is a pure function of its creation index —
  origin (0, 0), one 32×32 step per subsequent toplevel, wrapping after 8 — assigned
  once when the role is created (so unmap/remap returns to the same spot). No
  clock, no randomness, no dependence on the rest of the scene.
- **Reasoning:** Placement is policy and the policy daemon is M4; until then C10
  requires *some* answer, and the only property that must hold today is
  determinism, because goldens depend on it. The wrap exists because the core does
  not know the output size until the DRM backend (M2), so the cascade cannot clamp
  to a screen. Origin (0, 0) also kept the T3 shm goldens byte-identical across
  the migration — the first toplevel lands where the raw-commit path put content.

### Protocol errors are asserted by code; the rig gains that capability once

- **Source:** M1 T5; `crates/harness/src/protocol_rig.rs`
  (`RigProtocolError`, `expect_protocol_error`).
- **Affects:** the harness rig; all future conformance work.
- **Decision:** The rig observes the server's protocol error (code, interface,
  message) through the client connection's error state, and conformance tests
  assert the **specific code**, never merely "the client was disconnected".
  Recorded deviation: for a buffer committed before `ack_configure`, Smithay's
  `ensure_configured` posts `xdg_surface.not_constructed` (1) where the spec has a
  dedicated `unconfigured_buffer` (3); the `xdg_surface` object is not reachable
  through Smithay's public API, so we send Smithay's code and the test pins it.
- **Reasoning:** A compositor that kills a client for the wrong reason is still
  broken, so disconnection alone proves nothing. Building the capability once, at
  the rig level, is what makes every later conformance test cheap.

## 2026-07-25 — Seat, input, and the nested backend (M1 T6)

### Input reaches the core through one funnel; the nested backend's winit loop is T-input's stand-in

- **Source:** M1 T6 (prompt 09); `docs/scene_graph_v1.md` §11.1–§11.2; the §7
  honesty clause in `docs/plans/m1_tasks.md` T6.
- **Affects:** `crates/core/src/input.rs` (`InputEvent`), `protocol.rs`
  (`ProtocolHost::input`, seat application), `crates/backend-winit/` (new crate);
  `CORE-BOUNDARY.md` §7 (deviation recorded, spec **not** amended), I-2.
- **Decision:** Every input source produces the same `InputEvent` — evdev key
  codes, `BTN_*` buttons, output-space coordinates, **no Smithay type** — and
  hands it to `ProtocolHost::input`, a non-blocking message into the dispatch
  thread, which applies it to the seat. In the nested backend, winit's main-thread
  event loop *is* T-input: it must own the main thread and delivers input intake
  and presentation together, so M1 has T-input's **interface** without T-input's
  **thread**. The real split arrives with libinput and DRM (M2), producing the
  same `InputEvent`s.
- **Reasoning:** §7's load-bearing rule is that protocol objects have one owning
  thread, and that still holds — the loop sends messages, it does not reach into
  compositor state. Pretending to a thread split winit forbids would be dishonest
  RT of the kind the milestone plan warns about; recording the deviation with its
  replacement named keeps it bounded rather than becoming folklore.

### Pointer routing reads a dispatch-thread replica, not a query into the scene

- **Source:** M1 T6; Roland's ruling when the two options were put to him;
  `docs/scene_graph_v1.md` §11.3.
- **Affects:** `crates/core/src/input.rs` (`FocusMap`, `Hit`), the map/unmap and
  commit paths in `protocol.rs`; invariants I-2 (satisfied) and I-5 (unaffected).
- **Decision:** The dispatch thread keeps a **read-mostly replica** of each mapped
  surface's rect and stacking order and answers "topmost" / "under the cursor"
  from it. The scene remains canonical; the replica is derived, reconstructible,
  and updated by the same code that publishes the map/unmap/resize to the scene.
  Its ordering is the snapshot's draw order (ascending `z`, ties by `SurfaceId`)
  by construction.
- **Reasoning:** The alternative — `SceneHandle::query` per pointer motion — is a
  synchronous cross-thread round-trip on the input path that can queue behind the
  render thread's snapshot request, which is precisely what **I-2** forbids. §7
  names this exact pattern for T-input ("focus routing table — read-mostly
  replica of canonical focus"). The cost is a second copy of two fields; the
  discipline that keeps it honest is that both writes happen in one place.

### Focus policy: keyboard follows the topmost mapped toplevel (C10, until M4)

- **Source:** M1 T6; `docs/scene_graph_v1.md` §11.3.
- **Affects:** `protocol.rs` (`refocus_keyboard`), `CORE-BOUNDARY.md` C10, §4 rule 4.
- **Decision:** Keyboard focus is the topmost mapped toplevel, recomputed on map
  and unmap; pointer focus is the topmost mapped surface under the cursor. Both
  are C10 fallbacks, module-documented as temporary; the reference policy daemon
  S1 takes them over in **M4**. Client `set_cursor` is accepted and ignored for
  rendering (the host desktop draws the cursor in nested mode; the cursor plane is
  M2).
- **Reasoning:** Focus *is* policy — a reasonable user might want click-to-focus or
  focus-follows-mouse — so §4 rule 4 exiles it. It lives in the core now for the
  same reason as default placement: a compositor nothing can be typed into is not
  a compositor, and C10 exists so the core stays usable with every server dead.

### CI gains its first stated system dependency: `libxkbcommon`

- **Source:** M1 T6; `.github/workflows/ci.yml` (header amended, not deleted).
- **Affects:** CI; the "no apt step, on purpose" rule from M0.
- **Decision:** CI installs `libxkbcommon-dev`. The M0 rule is amended rather than
  dropped: still no libwayland, no GPU packages, no display server, and the winit
  backend is built but never run.
- **Reasoning:** `wl_keyboard` cannot exist without an xkb keymap, and xkbcommon
  is what compiles one. **Discovery worth recording:** the dependency was already
  there implicitly — Smithay depends on the `xkbcommon` crate unconditionally, so
  every test binary has linked `libxkbcommon.so.0` since the protocol layer
  landed, and CI passed because `ubuntu-latest` happens to ship it. The change is
  therefore from *luck* to *contract*, which is the honest way to describe it.

## 2026-07-25 — `wl_output`, graceful shutdown, and the acceptance blocker (M1 T7)

### `wl_output` is implemented, not stubbed — one output, real mode, scale 1

- **Source:** M1 T7 (prompt 10, pre-authorized scope); `docs/scene_graph_v1.md` §12.1.
- **Affects:** `crates/core/src/protocol.rs` (`OutputManagerState`, `Output`,
  `OUTPUT_NAME`, `OUTPUT_REFRESH_MHZ`, `DEFAULT_OUTPUT_SIZE`,
  `ProtocolHost::set_output_size`), `CORE-BOUNDARY.md` C3 (the core's protocol
  surface grows by one global).
- **Decision:** One `wl_output` (plus `xdg_output`, which Smithay derives from the
  same state) with the backend's real size, 60 Hz, scale 1, zero physical size,
  and `wl_surface.enter`/`leave` on map/unmap. The backend restates the size on
  every resize.
- **Reasoning:** A real client asks about the screen before it draws, so this
  global is on the critical path for the milestone's goal. The prompt's rule —
  "do not stub protocols; an advertised-but-hollow global is a lie to every future
  client" — is the whole argument: a hollow output would be found out on the first
  frame. The two values we *cannot* honour yet are marked as such in the doc
  (refresh is a claim without a vblank; scale is 1 because we implement no
  scaling).

### `wl_data_device_manager` is implemented in M1; clipboard access is not yet a capability

- **Source:** M1 T7. `foot` 1.25 refuses to start without it
  (`err: wayland.c:1758: no clipboard available`, exit 230) — reproduced windowed
  and headless, with `wl_output` present. Reported to Roland as a stop-and-report
  with three options; **he chose to implement it properly.**
- **Affects:** `crates/core/src/protocol.rs` (`DataDeviceState`,
  `SelectionHandler`, `DataDeviceHandler`, the DnD grab handlers,
  `set_data_device_focus` in `refocus_keyboard`); `CORE-BOUNDARY.md` C3 (protocol
  surface) and **I-7** (see the debt below).
- **Decision:** The clipboard and drag-and-drop are implemented through Smithay's
  data-device machinery, and clipboard focus follows keyboard focus — only the
  focused client may set the selection, which is the protocol's own answer to "who
  may overwrite what the user copied". The alternative (advertise the global,
  answer with nothing) was rejected on T7's own rule: an advertised-but-hollow
  global is a lie to every future client, and it would have been *easy* and would
  have *looked* like success.
- **Reasoning:** The clipboard is not a shell feature; it is a service the display
  server owes every client, and real applications treat its absence as a broken
  compositor. It is protocol machinery with canonical per-seat state, so §4 puts
  it in the core.
- **Scheduled debt (I-7):** access is **ungated** — any focused client may read and
  write the selection. That is ordinary Wayland and it is correct for M1, but I-7
  ("no privileged operation without a grant attached to the client's security
  context") will eventually govern it, and a remote (Rayland) client must land on
  the restricted side. The capability machinery is C8 and arrives with M4. Written
  down here so it is a scheduled debt rather than an oversight discovered later.

### Termination signals end the loop through its normal path

- **Source:** M1 T7; the T6 session summary's recorded wart;
  `docs/scene_graph_v1.md` §12.2.
- **Affects:** `crates/backend-winit/src/shutdown.rs`, `parhelion-dev` (and its
  new `--headless` mode), `crates/harness/tests/dev_binary.rs`.
- **Decision:** SIGINT/SIGTERM set an atomic flag; the event loop polls it and
  exits normally, so `Drop` unlinks the socket and its lock file. The handler
  itself does nothing else. `parhelion-dev --headless` runs the same compositor
  without a window so this is testable where there is no display.
- **Reasoning:** A signal handler may safely do almost nothing — cleaning up *in*
  the handler is how one gets a deadlock in a crash path. Asking the program to
  end the way it already knows how is both smaller and more correct. `--headless`
  is not a second compositor; it is the same one minus winit, and it exists so
  the claim "shutdown leaves no litter" is checked by CI rather than asserted.

## 2026-07-25 — Clipboard v1, and the limits of "advertise only what we honour" (M1 T7b)

### Clipboard v1 is core-protocol selection semantics, focus-gated

- **Source:** M1 T7b (prompt 11); Roland's option (a) after T7's blocker;
  `docs/scene_graph_v1.md` §12.2.
- **Affects:** `crates/core/src/protocol.rs` (`DataDeviceState`,
  `SelectionHandler`, `set_data_device_focus` in `refocus_keyboard` and
  `refresh_selection`); `CORE-BOUNDARY.md` C3, I-7.
- **Decision:** Offers flow only to the keyboard-focused client, and only the
  focused client may set the selection. **The protocol's own shape is the v1
  capability model**, and it satisfies I-7's letter: the grant is "has keyboard
  focus", checked in the core at request time. The deeper design — security-context
  restrictions on selection access, clipboard managers, primary selection, and a
  smaller grant set for remote (Rayland) clients — is **deferred to M4's capability
  work** (C8), where it gets a pointer from the dialect spec's capability section
  when that milestone slices.
- **Reasoning:** The clipboard is a service the display server owes every client,
  not a shell feature — `foot` will not start without it. Focus-gating is not a
  placeholder for a capability check; it *is* the check the protocol defines, and
  every real clipboard tool is built around it (`wl-copy` maps a toplevel purely to
  obtain focus, sets the selection, and destroys the window again — observed with
  `WAYLAND_DEBUG=1`).

### Drag-and-drop is refused immediately rather than half-implemented

- **Source:** M1 T7b; `docs/scene_graph_v1.md` §12.2.
- **Affects:** `ClientDndGrabHandler::started` (cancels the source), the
  `starting_a_drag_cancels_the_source_rather_than_hanging` rig test.
- **Decision:** A client that starts a drag has its source **cancelled at once**.
  Protocol-legal (a compositor may cancel a drag whenever it likes), and the client
  learns immediately instead of waiting on a drag that will never produce an enter,
  a drop, or a cancel.
- **Reasoning:** A real drag is a **pointer grab**, and how grabs compose with
  Parhelion's focus model — C10 today, S1's policy in M4, and eventually shaped and
  3D-transformed windows — is a design conversation, not an afternoon's plumbing.
  Smithay would supply the grab machinery; the compositor-side semantics are ours
  to decide. Half of that, shipped quietly, is the same lie the clipboard stub would
  have been.

### "Advertise only what we honour" met a client that requires the lie — reported, not resolved

- **Source:** M1 T7b task 2, measured; see the T7b session summary for the traces.
- **Affects:** `wl_subcompositor` advertisement (unchanged, deliberately);
  `crates/harness/tests/conformance.rs` (the advertised set is now pinned).
- **Finding, in three parts:** (1) The global **is** separable —
  `CompositorState::subcompositor_global()` plus `DisplayHandle::remove_global`
  withdraws it, and that was implemented and tested. (2) Withdrawing it makes
  Parhelion **unusable by real clients**: `foot` refuses to start
  (`err: wayland.c:1746: no sub compositor`, exit 230), which fails the milestone's
  own acceptance criterion. (3) `foot` binds the global but calls `get_subsurface`
  **zero** times in a full session (`WAYLAND_DEBUG=1`), so today the gap is
  *dormant* for the clients we run.
- **Status: the advertisement stays, as a stated debt**, because the alternative
  breaks M1's acceptance and the ecosystem. The honest resolution is to implement
  subsurfaces (scene work, a slice of its own, likely alongside popups) — **this is
  Roland's call and is not taken here.** The conformance test now pins the exact set
  of advertised globals so the next change to it is deliberate.
- **Reasoning:** The principle is right and it is *not* being abandoned: it was
  applied to `wl_data_device_manager` (implemented rather than stubbed) in the same
  session. What this entry records is that the principle, applied mechanically to a
  global the whole ecosystem probes for, produces a compositor nothing will talk to
  — which is a worse outcome than a documented, dormant, scheduled gap.

## 2026-07-25 — Advertise-before-support, refined (M2 T0)

### Advertise-before-support requires loud refusal at point of use — never silent wrongness

- **Source:** prompt 12 / chat review of T7b; Roland's resolution of the T7b
  Pending item.
- **Supersedes:** "advertise only what we honour" **in its absolutist reading**
  (2026-07-25, T7b). The principle stands; the mechanical application does not.
- **Affects:** the protocol layer (`crates/core/src/protocol.rs`), T7b's pinned
  advertised-globals test, `docs/scene_graph_v1.md` §12.3.
- **Why the absolutist reading fails:** T7b measured it. Clients hard-gate on the
  *presence* of globals they never *use* — `foot` binds `wl_subcompositor`, calls
  `get_subsurface` **zero** times, and refuses to start without the global.
  Withdrawal fails honest clients at the door; silent non-support renders wrong.
  Neither is acceptable.
- **Decision:** A global may be advertised ahead of support **only if every
  unsupported request on it posts a protocol error with a clear message.** The
  client is told, at the exact moment it asks for the thing, that the thing does
  not exist yet.
- **Known cost, accepted:** this converts *degraded* into *dead* for clients that
  actually create subsurfaces (toolkits use them for tooltips, popups, CSD
  shadows). That is the right trade for a development compositor — a dead client
  with a diagnostic beats a live client rendering a lie — and it is temporary:
  retired when subsurfaces land (**M2 T7**).
- **Applied to:** `wl_subcompositor` now.

### Client intake is per-client readiness sources; throttling deregisters

- **Source:** M2 T0 (prompt 12), paying T2's recorded M2 promise;
  `docs/scene_graph_v1.md` §8.5.
- **Affects:** `crates/core/src/protocol.rs` (`admit_client`,
  `dispatch_one_client`, `update_throttles`, `ClientSource`, `State::display`);
  invariant I-10's fairness rider; the spike's shard-count-agnostic requirement.
- **Decision:** At `add_client` the client's socket is `try_clone`d and *that*
  descriptor is registered as its own `calloop` source; readiness dispatches that
  client alone. wayland-backend's aggregate poll fd is **no longer watched**
  (grep-verifiable: no `Generic::new(display, …)`). Throttling is then literal —
  the client's source is **disabled** — and re-armed below
  `RESUME_PENDING_FRAME_CALLBACKS` (a quarter of the throttle mark; hysteresis, so
  a steady flooder does not toggle its registration every tick).
- **Reasoning:** v1 could only *skip* a throttled client, because the backend
  keeps every client socket inside one epoll fd and exposes no way to deregister
  one; that fd therefore stayed ready and the dispatch loop busy-waited. Owning a
  second descriptor is what makes a registration we control. **The spin ends by
  construction, not by bounding** — measured: the old semantics turn the loop
  100 046 times in 300 ms, the new one ~15.
- **Rejected, recorded so they stay rejected:** edge-triggering the aggregate fd
  (stops the spin, starves shard-mates — the aggregate never goes quiet while a
  throttled client holds data, so other clients' readiness produces no edge);
  timer-based rate limiting (bounds the spin without ending it, and would let an
  "iterations bounded" test certify the wrong thing).
- **Shard-readiness:** per-client sources are the objects a future shard takes
  ownership of — a shard becomes "these clients' sources plus their `Display`".
  This moves toward the spike's interface requirement rather than bending it.

### CORRECTION (M2 T0): foot *does* use subsurfaces — the entry above rests on a bad measurement

- **Source:** M2 T0, implementing the tripwire the entry above requires.
- **What was wrong:** T7b reported that `foot` "binds `wl_subcompositor` but calls
  `get_subsurface` **zero** times". That was my error — a `WAYLAND_DEBUG` grep
  written as `wl_subcompositor@N.get_subsurface` where the debug format uses `#`,
  so it matched nothing and I read the empty result as evidence of absence. The
  claim propagated into the T7b session summary, the T7b decision entry, this
  prompt's superseding entry, and the code comments.
- **What is actually true**, measured with the corrected pattern: `foot` creates
  **nine** subsurfaces during startup and attaches buffers to **eight** of them.
  They are its client-side decorations — title bar, borders, corners.
- **What this overturns:** the premise "clients hard-gate on the *presence* of
  globals they never *use*". foot both requires the global **and** uses it for
  content. The reasoning in the entry above therefore does not apply to
  `wl_subcompositor`, which was its only application.
- **What was tried, and measured, before reporting:** both candidate refusal
  points were implemented. Refusing `get_subsurface` kills foot at startup;
  refusing a subsurface that *commits a buffer* (the narrower rule — refuse where
  content would actually be dropped, which foot's pixel-less input-region
  subsurface would have survived) kills it a moment later, because eight of its
  nine subsurfaces carry pixels. **There is no refusal point that keeps an honest
  client alive.**
- **Status: the tripwire is not implemented, and the debt stands.** Loud refusal
  and a working terminal are mutually exclusive until subsurfaces are real
  (M2 T7), and the milestone's acceptance criterion *is* the terminal. The
  advertise-before-support principle above is untouched and still governs any
  *future* global; it simply has no applicable instance today.
- **Consequence, now visible and stated:** foot renders **without its
  decorations** — they are subsurfaces we silently drop. That is exactly the
  silent wrongness the principle forbids, and it stands until T7. It also means
  the interactive smoke will show an undecorated terminal; that is this debt, not
  a new bug.

## 2026-07-26 — Subsurfaces v1: the debt discharged (M2 T7)

### The scene owns a tree; the renderer stays a flat list

- **Source:** M2 T7 (prompt 13, pulled to the front of M2);
  `docs/scene_graph_v1.md` §12.3.
- **Affects:** `crates/core/src/scene/node.rs` (`parent`, `children`,
  `NodeRole::Subsurface`), `scene/state.rs` (tree ops, `is_mapped`,
  `apply_commit`, flattening), `crates/core/src/protocol.rs` (effective-commit
  walk, input routing through the tree), the T0 conformance test (**inverted**).
- **Decision:** Subsurfaces are scene nodes with a parent and an ordered child
  list that includes the parent's own slot; a child's transform is
  parent-relative; the snapshot flattens the tree to composition order. Mapping is
  transitive (a subsurface is mapped iff its whole ancestor chain is). One
  effective commit produces **one** scene message, so a synchronized batch is
  atomic by construction. **No renderer change** — it still consumes a flat
  back-to-front list.
- **Reasoning:** The scene is canonical state (I-5), so tree semantics belong
  there rather than in a pre-flattened message from the protocol side: damage,
  hit-testing, and the mapping law are all tree questions, and answering them in
  one place is what keeps input and pixels agreeing about who is on top. Keeping
  the renderer flat means the whole feature cost the compositor nothing
  structurally — the seam held.
- **Measured consequence worth recording:** a subsurface's position is re-stated
  on every effective parent commit, so damaging unconditionally on `set_position`
  repainted 76% of the output per keystroke in the acceptance run. The no-op check
  is not an optimisation; it is the difference between damage tracking working and
  not.

### Closing coda on the advertise-before-support chain

- **The arc:** T7b measured `foot` as binding `wl_subcompositor` without using it,
  and proposed withdrawing the global. T0 found that measurement was wrong (nine
  subsurfaces, eight with buffers), that **no** refusal point could keep an honest
  client alive, and pinned the silent wrongness in a test rather than papering
  over it. T7 implements the feature, and that test **inverts**: the same
  assertion, opposite direction.
- **What the chain is worth keeping for:** the principle survives its own failed
  application. "Advertise-before-support requires loud refusal at point of use"
  still governs any future global — it simply had no instance it could be applied
  to, because its one candidate was a global clients both require *and* use. The
  real answer to an unimplementable refusal was never a cleverer refusal; it was
  implementing the thing.
- **Effect:** foot renders **with its decorations**, and the acceptance test
  asserts they composite. The Pending item for subsurfaces is struck.

## 2026-07-26 — Session, DRM/KMS atomic, dumb buffers (M2 T1)

### T-commit owns the metal; the render tick becomes a message, and no backend trait is introduced

- **Source:** M2 T1 (prompt 14); `docs/scene_graph_v1.md` §13.1.
- **Affects:** `crates/backend-drm/` (new crate, new workspace member),
  `CORE-BOUNDARY.md` §7 (T-commit — **satisfied, not amended**) and §3 C1.
- **Decision:** A dedicated `parhelion-commit` thread owns the libseat session,
  the DRM fd (and DRM master), the atomic surface, both scanout buffers, and the
  vblank source, in its own `calloop` loop. `parhelion-render` runs the existing
  `RenderLoop` on its own thread, ticked **by a message from T-commit once per
  vblank**. The backends therefore differ only in *who calls `tick`* — and so
  **no backend trait was added**. The prompt allowed the interface to grow if it
  needed to; it did not. The core already has the seam that matters (the
  `Compositor` trait, §6), and a trait over two callers with different
  lifecycles would have been abstraction beyond the task.
- **Reasoning:** §7 names T-commit's ownership set almost exactly, and the value
  of writing it down was always that one thread would eventually have to hold it.
  Making the tick a message rather than a trait method keeps the headless and
  nested tick sources **byte-for-byte unchanged**, which is what lets the whole
  existing suite go on proving what it proved. The §11.2 winit deviation is
  untouched and still stands until T2.
- **Discovered, and load-bearing:** Smithay's `LibSeatSession` **and** its
  notifier are `Rc`-based and therefore **not `Send`**. The session cannot be
  created on one thread and handed to another, so *all* hardware setup happens on
  T-commit and the discovered mode travels back over a channel. This is why the
  startup sequence is "spawn, then learn the mode, then build the compositor"
  rather than the reverse.

### Pixels cross the thread boundary, not `Frame`s

- **Source:** M2 T1; `docs/scene_graph_v1.md` §13.3.
- **Affects:** the T-render → T-commit channel; `backend-drm/src/present.rs`.
- **Decision:** T-render converts its composited frame to `XRGB8888` into a
  **recycled** `Vec<u8>` and sends that; T-commit copies it row-wise into the back
  buffer's mapping and page-flips. The buffer returns with the next tick, so
  steady-state allocation is zero.
- **Reasoning:** The prompt's parenthetical put the copy on T-commit ("hands
  completed `Frame`s over a channel"). It cannot be done that way: the CPU
  compositor **retains** its frame for damage tracking (§9.4), so the frame cannot
  be moved out from under it, and cloning it would add a whole second full-frame
  copy. Converting on the thread that just touched every pixel costs one pass —
  work that has to happen regardless — and leaves T-commit one memcpy per row.
  One pass, one copy, is the minimum for two threads that must not share memory.
- **Recorded consequence:** `blit_to_pitch` and `frame_to_xrgb8888` duplicate the
  shape of `backend-winit`'s `present::frame_to_argb`. Three similar lines beat a
  premature abstraction (CLAUDE.md), and the two differ in target format and
  stride handling; unifying them is a cleanup for whenever a third presenter
  appears.

### Dumb buffers go straight to `drm`; Smithay's allocator wrapper cannot map what it allocates

- **Source:** M2 T1; `crates/backend-drm/src/buffer.rs`.
- **Affects:** the consume/bypass layer table (decision "Smithay threading fit"
  entry 2 — **confirmed and extended**, not amended).
- **Verdict on the seam check the prompt asked for: clean.** `backend_drm` +
  `backend_session_libseat` build with `default-features = false` and pull **no**
  `backend::renderer`, no `backend_gbm`, and no `backend_egl`.
  `smithay::backend::renderer` appears nowhere in the workspace (grep-verifiable).
- **Decision:** Dumb-buffer creation, mapping, and framebuffer attachment use
  `smithay::reexports::drm` directly rather than `smithay::backend::allocator::dumb`.
  Smithay's `DumbBuffer` exposes only `handle(&self) -> &Handle` while
  `map_dumb_buffer` requires `&mut` — so the wrapper **cannot map the buffer it
  allocated**, and writing into that mapping is the one thing this backend does
  every frame.
- **Reasoning:** Four ioctls of our own are smaller and more readable than a
  workaround, and they keep the per-frame path something a reviewer can follow.
  This is not a bypass of the layer decision; it is the decision's own rule
  ("consume the frontend and the hardware layers, supply our own pixels") landing
  on a wrapper that does not fit.

### `wl_output`'s refresh comes from the mode's timings, not from `vrefresh`

- **Source:** M2 T1; `docs/scene_graph_v1.md` §13.2. **Retires** the scheduled
  claim in the T7 entry "`wl_output` is implemented, not stubbed".
- **Affects:** `crates/core/src/protocol.rs` (`Control::OutputMode`,
  `ProtocolHost::set_output_mode`, `OUTPUT_REFRESH_MHZ`'s meaning),
  `crates/backend-drm/src/mode.rs`.
- **Decision:** `ProtocolHost::set_output_mode(w, h, refresh_mhz)` joins
  `set_output_size`, and the DRM backend computes the refresh as
  `clock_kHz × 10⁶ / (htotal × vtotal)` in millihertz — the kernel's own formula,
  with interlace/double-scan/vscan handled — rather than reading
  `drm_mode_modeinfo.vrefresh`. `OUTPUT_REFRESH_MHZ` survives as the **default for
  backends with no vblank** (nested, headless), which is an honest use of it.
- **Reasoning:** `vrefresh` is whole hertz. Rounding a panel's 59.953 Hz to 60 is
  precisely the plausible-looking lie T7 was forced into and this task exists to
  retire; clients schedule against the advertised rate, so a 0.08% error is a
  frame of drift every twenty minutes. Degenerate timings return `None` and the
  caller substitutes the default **and says so on stderr** — a wrong number
  delivered confidently is worse than a stated fallback.
- **Connector/mode policy v1 (recorded so it is not re-litigated):** first
  connected connector with any mode, at its preferred mode; without a preferred
  mode, largest area then higher refresh then earlier index — a **total** order,
  because two runs picking differently would make "it looked right yesterday"
  evidence of nothing. Multi-output, hotplug, and non-preferred modes are **M9**.

### The re-stated-state rule is named, and scoped to where it is actually enforced

- **Source:** M2 T1, codifying T7's 76% measurement; `docs/scene_graph_v1.md` §9.3.
- **Affects:** the damage section as a named rule; every future setter.
- **Rule:** *State re-stated by the protocol must be a no-op when it has not
  changed. Every setter that damages compares first, and damages only on a real
  difference.*
- **Reasoning:** Wayland restates state constantly — a subsurface's position on
  every effective parent commit, buffer scale/transform with every attach — so a
  setter that damages unconditionally turns every commit into a structural one and
  damage tracking silently stops tracking. Both versions are *correct* (§9.1
  survives, because over-approximation is always legal), which is exactly why it
  must be a stated rule rather than something a test catches: the proportionality
  tests notice it as a number drifting, not as a failure.
- **Scope, stated honestly:** the rule is enforced today in exactly one place,
  `set_subsurface_position`. `set_geometry`, `set_z`, `set_source`, and `set_role`
  damage unconditionally and that is *presently* harmless — nothing restates them;
  they are reached only from `place_solid` and from genuine xdg transitions. That
  is a property of today's call graph, not of those functions, so the rule carries
  an obligation: **a setter that acquires a per-commit caller acquires the check in
  the same change.**

### CI gains its second stated system dependency: `libseat`

- **Source:** M2 T1; `.github/workflows/ci.yml` (header extended, not replaced).
- **Affects:** CI; the M0 "no apt step" rule, amended once more.
- **Decision:** CI installs `libseat-dev`, **builds** `parhelion-backend-drm`, and
  runs its unit tests — all of which are pure functions over plain data (mode
  policy, refresh arithmetic, stride and channel-order handling). It never opens a
  device, takes DRM master, or touches a connector.
- **Reasoning:** Unlike `libxkbcommon` (which turned out to be linked implicitly
  already — luck made contract), this one is a genuinely new requirement: Smithay's
  libseat session links against the library, so the crate does not build without
  the headers. The rule that still holds: no libwayland, no GPU packages, no
  display server, and no hardware path ever executed in CI. What proves the
  hardware path is `docs/plans/m2_t1_smoke_checklist.md` and Roland's eyes, and the
  session summary says so item by item.

## Pending

- Lock-screen fail-locked design (`CORE-BOUNDARY.md` §6 note).
- Adoption of ENO's project-index + sessions/ + diary structure:
  agreed in principle; instantiate at repo creation.
