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

## Pending

- Lock-screen fail-locked design (`CORE-BOUNDARY.md` §6 note).
- Adoption of ENO's project-index + sessions/ + diary structure:
  agreed in principle; instantiate at repo creation.
