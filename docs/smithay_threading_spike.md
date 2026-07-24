# Smithay Threading-Fit Spike — Report

> **Re-entrancy header.**
> **Status:** complete · **Date:** 2026-07-24 · **Kind:** investigation-spike report (M0 task 2).
> **Upstream:** `CORE-BOUNDARY.md` §7 (threading model), §4 (placement), I-1/I-2/I-3; decision-log "Pending: Smithay threading fit".
> **Downstream:** `crates/core/` protocol layer, M0 task 3 (headless backend + protocol harness).
> **Deliverable kind:** a report and a recommendation. **The decision is Roland's.** A drafted decision-log entry sits at the end of this document (§8), *not yet appended* to the log.
> **Reproduce:** `cd tools/spikes/smithay-threading && cargo run` (prints Q1 static-fact confirmation and the Q2 split-experiment trace). Spike source: `tools/spikes/smithay-threading/`.

---

## 0. One-sentence recommendation (for the yes/no)

**Consume Smithay as the protocol frontend and (later) its hardware backends, bypass its renderer and desktop/space abstractions, and drive it at `shards = 1` behind a `ProtocolHost` interface that makes shard count an implementation detail — this fits CORE-BOUNDARY §7 with no conflict.**

If you agree, §8's decision-log entry lands as a small edit; no further session needed.

---

## 1. Version pins (evidence baseline)

Resolved by `cargo` in this environment on 2026-07-24; recorded verbatim in the committed `tools/spikes/smithay-threading/Cargo.lock`. These are the exact versions every claim below was tested against.

| Crate | Version | Notes |
|-------|---------|-------|
| `smithay` | **0.7.0** | Current crates.io release; latest as of the spike (published 2025-06-24). |
| `wayland-server` | 0.31.14 | The protocol frontend Smithay builds on. |
| `wayland-backend` | 0.3.16 | Default = pure-Rust (`rs`) backend; `sys`/libwayland is opt-in. |
| `wayland-client` | 0.31.15 | Used by the spike's scripted client and by the future protocol harness. |
| `wayland-scanner` | 0.31.11 | Code-gen (proc-macro). |
| `wayland-protocols` | 0.32.13 | Extension protocol types. |
| `wayland-sys` | 0.31.11 | Pulled but **not linked** unless a `*_system` feature is on (it is not). |
| `calloop` | 0.14.4 | Smithay's event-loop substrate. |

Two environment facts worth recording as evidence in their own right:

- **Smithay 0.7 builds headless with no system Wayland libraries.** The default `wayland-backend` is the pure-Rust `rs` backend (`wayland-backend-0.3.16/src/lib.rs:83` selects `rs::server` unless `server_system` is set). The spike compiled and ran with zero `libwayland-*`/pkg-config presence. This matters for CI (M0 task 3): the protocol layer needs no C Wayland stack.
- **Smithay 0.7 is the current crates.io release and has been for ~13 months** (0.7.0 = 2025-06-24; the spike ran 2026-07-24). Master carries an `Unreleased` section. See §5.

---

## 2. Method

Evidence, not opinion (CLAUDE.md, technical attitude). Three kinds were used:

1. **Compiler as oracle.** Static `Send`/`Sync` assertions (`fn assert_send<T: Send>()`) on the real types. A false claim would be a compile error; the binary building *is* the proof.
2. **Source reading** of the pinned `wayland-server-0.31.14` / `wayland-backend-0.3.16` in the local cargo registry, cited by file:line.
3. **A runnable split experiment** (`tools/spikes/smithay-threading/src/main.rs`) that stands up the §7 dispatch/scene thread split with one scripted client and prints a pass/fail trace.

No `crates/*` was touched. The spike is its own cargo workspace (empty `[workspace]` table in its manifest), so `make test` at the repo root never compiles it — confirmed green (`make test` → 0 failed, clippy clean).

---

## 3. Findings

### Q1 — Protocol dispatch threading: can dispatch be sharded across threads with per-client ordering preserved?

**Send/Sync facts (all proven by compilation; see `q1_static_facts` and the `NotSendState` probe):**

| Type | `Send` | `Sync` | Evidence |
|------|:---:|:---:|----------|
| `Display<State>` | ✅ | ✅ | **Unconditional** — holds even for a `State` containing a raw pointer (`Display<NotSendState>: Send` compiled). |
| `DisplayHandle` | ✅ | ✅ | `wayland-backend`'s handle is `Arc<Mutex<dyn ErasedState + Send>>` (`rs/server_impl/handle.rs:89`). |
| `ClientId` / `ObjectId` / `GlobalId` | ✅ | ✅ | Auto-derived; ride in cross-thread messages. |

The load-bearing discovery: **`Display<State>` is `Send + Sync` independent of `State`.** The compositor `State` is *not stored inside* the `Display` — it is borrowed per call via `dispatch_clients(&mut self, state: &mut State)` (`wayland-server-0.31.14/src/display.rs:57`). `State` appears in the backend only erased behind `Arc<dyn ObjectData<State>>`, and the erased-state trait object carries a `+ Send` bound, which is what makes the whole handle unconditionally thread-safe. There is **zero `unsafe impl Send/Sync` in the `rs` backend** (grep: 0 hits) — it is all honest auto-derivation.

`#discovery` — this is stronger than "Smithay is single-threaded." The protocol machinery is freely thread-movable; the only real constraint is the *idiom*, discussed next.

**What sharding is and isn't possible:**

- **Within a single `Display`: dispatch is serial.** `dispatch_clients`/`dispatch_single_client` take `&mut self`; you cannot dispatch two clients of the same `Display` concurrently. (`dispatch_single_client` exists — `wayland-backend/src/server_api.rs:599` — but is rust-backend-only and still serial.)
- **Across `Display` instances: sharding is fully possible.** Each `Display` is independent and `Send`. The concrete recipe, all from stable public API:
  1. One acceptor owns a `ListeningSocket` (`wayland-server/src/socket.rs:19`); `accept()` yields a bare `UnixStream` (`socket.rs:144`).
  2. The acceptor hands each accepted stream to a chosen shard thread, which calls `DisplayHandle::insert_client(stream, data)` (`display.rs:105`) on *its own* `Display`.
  3. Each shard thread runs its own dispatch loop over its own `Display` + `State`.
  - **Per-client ordering is preserved for free:** a client lives entirely inside one `Display` (its object IDs and socket are bound there), so exactly one thread ever dispatches it. Cross-client independence is exploited across shards. This is precisely §7's T-proto[n] contract.

**Where the global-`State` coupling actually bites** (the prompt's real question):

- It does **not** force protocol state and scene/render state into one thread. Callbacks receive `&mut State` where `State` is *whatever we choose* — it can be a thin protocol-only struct holding a `Sender` to the scene. Proven in Q2.
- It **does** mean: (a) each shard has its *own* `State` and its *own* set of advertised globals, so shared canonical state (scene, focus, capabilities) must be reached by messages, never a shared `&mut` — which §7 already mandates; (b) a client cannot migrate between shards after connect; (c) the `sys`/libwayland backend lacks `dispatch_single_client` and is less flexible — stay on the default `rs` backend.

**Verdict Q1:** Sharding across `Display` instances is possible today with per-client ordering preserved, using only stable API. Single-`Display` internal parallelism is not (and Wayland's per-client ordering guarantee means you wouldn't want it). No §7 conflict.

### Q2 — Is the dispatch/scene split real? (the experiment)

**Yes — it compiles and runs.** `tools/spikes/smithay-threading/src/main.rs`, `cargo run`, exit 0:

```
[Q2] split experiment: dispatch thread + scene thread + 1 client
[client] bound wl_compositor
[client] created wl_surface
[client] committed wl_surface
[scene] surface created: ObjectId(wl_surface@4[0], 4)
[scene] surface committed: ObjectId(wl_surface@4[0], 4)
[scene] client gone
[scene] channel closed — final toy scene: 1 surface(s), 1 commit(s)
[Q2] RESULT: split ran — commit flowed dispatch-thread -> scene-thread. PASS
```

Structure, mapped to §7:

- **Dispatch thread (`run_dispatch`)** owns the Smithay `Display<Protocol>` + `Protocol` state and pumps `dispatch_clients` / `flush_clients`. `Protocol` holds *only* a `Sender<SceneMsg>` and a done-flag — no scene data. Its `Dispatch::request` callbacks translate `wl_compositor.create_surface` and `wl_surface.commit` into `SceneMsg`s and send them. → T-proto[n].
- **Scene thread (`run_scene`)** owns a toy scene (a `Vec` of surface IDs) and mutates it *only* from received messages. **No Wayland type is in scope in that function** — the isolation is structural, not conventional. → the scene-graph owner.
- **The edge between them is a channel** carrying `Send` tokens (`ObjectId`). No shared lock, no synchronous call. This is the §7 "publishes state changes to the scene graph via messages" edge, and it satisfies I-3 by construction (the scene never calls back synchronously into the protocol thread).
- **One scripted client (`run_client`)**, a real `wayland-client` connection over a `socketpair`, binds `wl_compositor`, creates a surface, commits, round-trips.

The experiment deliberately uses the smallest real global (`wl_compositor` → `wl_surface`) so the commit path — the exact moment a real core folds pending state into canonical state — is what crosses the thread boundary.

**Verdict Q2:** The split is not merely allowed; it is the natural shape. Smithay's `State` idiom couples *protocol object callbacks* to a `State`, but that `State` is free to be a message-publisher, keeping scene/render ownership on a different thread.

### Q3 — Layer selection (independent of the dispatch verdict)

Smithay 0.7 is a *toolbox*, feature-gated (`smithay-0.7.0/Cargo.toml [features]`); we take modules à la carte. Modules: `backend` (`drm`, `gbm`/`allocator`, `egl`, `libinput`, `session`, `udev`, `vulkan`, `winit`, `x11`), `renderer` (`gles`, `glow`, `pixman`, `multigpu`, plus `damage`/`element`/`sync`), `desktop` (Space/Window), `input` (Seat), `output`, `utils`, `wayland` (protocol handlers + `delegate_*` macros), `xwayland`.

| Layer | Disposition | One-line reason |
|-------|-------------|-----------------|
| **Protocol frontend** — `wayland-server` re-export + `smithay::wayland::*` handlers, `delegate_*` macros | **Consume** | The whole point; canonical-state ownership + per-client ordering live here (C3). Compatible with §7 (Q2). |
| **Serials / geometry / `utils`** | **Consume** | `Serial`, `Rectangle`, `Point`, `Logical`/`Physical` coordinate spaces are unopinionated and save re-invention. |
| **`backend` — DRM/KMS, `session`/libseat, `libinput`, `udev`, `egl`, `allocator`/gbm** | **Consume (M2)** | Hardware plumbing we must not rewrite; each is a `calloop` event source (see §5). Frame-path code (C1/C2/C9) but the *mechanism* is generic. |
| **`backend::winit`** | **Consume (M1)** | Nested dev backend for free. |
| **`allocator` dmabuf + `renderer::sync` (syncobj)** | **Consume-with-wrapper** | We need dmabuf import + explicit sync (C9, I-11), but wrapped so our renderer owns the GPU context (C5/T-render). |
| **`renderer` traits (`Renderer`/`Frame`/`gles`/`element`/`damage`)** | **Bypass** | Parhelion has its own 3D-native renderer; Smithay's element/damage model is 2.5D-surface-oriented and would fight the 3D scene graph and the regime machine (C5/C6). This is also where Smithay churns most (§5). |
| **`desktop` (Space/Window/layer map)** | **Bypass** | A 2D stacking-and-placement helper = window-management *policy* → server per §4/I-6; and it presumes a surface≈window model Parhelion's scene graph replaces. |
| **`input` (Seat/keyboard/pointer focus bookkeeping)** | **Consume-with-wrapper** | Serial/focus/keymap bookkeeping is genuinely fiddly and worth reusing, but routing is T-input-owned (I-2) — wrap, don't cede control. |
| **`xwayland`** | **Consume (M9)** | X1 is a confined server; Smithay's XWayland bridge is a sensible starting point. |
| **`backend::vulkan`, `renderer::multigpu`** | **Bypass / revisit** | Our renderer decides its own GPU abstraction; multigpu is an open question (CORE-BOUNDARY §10.6), not an M0–M2 commitment. |

Net shape: **Parhelion consumes Smithay's "north" (protocol) and eventually its "south" (hardware backends), and supplies its own "middle" (scene graph + renderer + regime machine).** That is exactly the seam that keeps the 3D-native ambition unconstrained.

### Q4 — Evolution risk

**Cadence (crates.io `created_at`):** 0.1.0 (2017) · 0.2.0 (2019) · 0.3.0 (2021) · **~3.5-year git-only gap** · 0.4.0 (2025-01-23) · 0.5.0 (2025-02-27) · 0.6.0 (2025-04-25) · **0.7.0 (2025-06-24)**. So: a burst of ~2-month breaking releases through 2025, then 0.7.0 has held for ~13 months with changes accumulating on master (`Unreleased`).

**Where the churn is (CHANGELOG):**
- 0.7.0 breaking changes: **DRM/syncobj/framebuffer** (`DrmSyncobjHandler`, `DrmTimeline::new`, `GbmFramebufferExporter`).
- 0.6.0: **renderer** refactor — `ContextId` newtype across renderers, client scale `u32 → f64` (fractional scaling), iterator return types.
- Nothing in recent releases touches threading, `Send`/`Sync`, calloop, or per-client dispatch. 0.4.0 established "most backends are `calloop` event sources"; that's stable.

**Reading:** the breakage is concentrated in exactly the layers Q3 says to **bypass or wrap** (renderer, DRM). The layer we depend on hardest — the protocol frontend — is the most stable, and its foundation (`wayland-server` 0.31.x) is a separately-versioned, mature crate. That de-risks the dependency: our largest surface area sits on the calmest layer.

**Downstream reference (cosmic-comp):** historically pinned Smithay by **git rev**, and its recorded pain (GitHub issues #21/#44/#798/#1115) is overwhelmingly `wayland-backend` version-resolution conflicts and git-source breakage — i.e. the cost of tracking `master`, not of a semver release. Now that Smithay publishes to crates.io, **pinning to `smithay = "=0.7.0"` (exact) with a committed `Cargo.lock`** avoids that entire class of problem.

**Pinning recommendation:** exact-version pin (`=0.7.0`) + committed lockfile; upgrades are deliberate, batched, and land behind the `ProtocolHost` wrapper so churn in bypassed layers can't reach core call sites. This mirrors the SPINE "vendored, pinned, deliberate import" discipline already adopted for the control plane.

*Speculation (labeled):* a future 0.8 is likely given the `Unreleased` accumulation; based on the 0.6/0.7 pattern it will most probably break renderer/DRM APIs again rather than the frontend. Not verified — no roadmap commitment was found touching the threading model.

---

## 4. Recommendation and tradeoffs

**Recommendation:** adopt Smithay as the **protocol frontend now** (M0/M1) and its **hardware backends later** (M1 winit, M2 DRM/session/libinput/udev); **bypass its renderer and `desktop` layers**; drive dispatch at **`shards = 1` today behind a `ProtocolHost` abstraction** whose interface makes shard count an implementation detail, not an architectural commitment. Pin exact + lockfile. **No CORE-BOUNDARY §7 conflict exists** — the acceptable "shards=1 now, interface allows sharding later" outcome the prompt anticipated is the correct one.

**The interface that keeps shard-count an implementation detail** (so growing from 1→N shards is a code change, never a design change):

1. **Client→shard assignment happens at accept time**, inside `ProtocolHost`. Nothing downstream may assume a single `Display`. At `shards = 1` there is one `Display`; the acceptor→shard hand-off (`ListeningSocket::accept` → `insert_client`) still exists as the seam.
2. **The scene / canonical state is reachable from protocol threads only via message channels** — never a shared `&mut` or a lock (§7, I-1, I-3). Already law; this spike confirms Smithay does not force a violation.
3. **Anything crossing to the scene is a `Send` token** (`ObjectId`, or a core-assigned `SurfaceId`), not a borrowed Wayland resource. Proven in Q2.
4. **Globals/capabilities are advertised per-`Display`**, identically across shards, so adding shards is invisible to clients (each shard offers the same global set; capability checks (I-7) run in the shard's `State` against canonical grants fetched by message).

**Tradeoffs:**

- **`shards = 1` (recommended now):** simplest; matches Smithay's idiom and the `anvil` reference; one dispatch thread. Risk: a single dispatch thread could bottleneck under many chatty clients — but **protocol dispatch is not the frame path**, so I-1/I-2 are unaffected, and the bottleneck is measurable before it's real. M1 already mandates the §7 thread skeleton "even if protocol shards = 1," so the seam is built regardless.
- **`shards = N` (deferred):** real cross-client parallelism, but N sets of globals/state, more resync surface (I-5), and the `sys`-backend limitation. Buy it only against measured contention.
- **Bypassing Smithay's renderer:** costs us the reuse of a working GLES renderer for the early milestones — but adopting it would couple the 3D scene graph to a 2.5D element model and to Smithay's most volatile API (§5). The bypass is what protects Thesis 1 (3D-native) and Thesis 3 (regime machine).

**What would change this verdict:** if profiling in M2+ shows single-thread dispatch missing its budget under realistic client counts, escalate 1→N shards (a `ProtocolHost` implementation change). If a future Smithay major rewrote `wayland-server`'s ownership such that `Display<State>` lost its unconditional `Send` (no sign of this), the sharding recipe would need re-checking — the static assertions in the spike are the regression test for that and should migrate into `crates/core` when the protocol layer lands.

---

## 5. Implications for M0 task 3 (headless backend + harness)

1. **Smithay is not on M0's critical path for *rendering*.** The headless backend renders a test pattern to memory ourselves (M0 scope); Smithay's role in M0/M1 is the protocol frontend only. M0 task 3 can proceed without any Smithay backend.
2. **The protocol test rig is essentially this spike, promoted.** The harness's "spawn a scripted Wayland test client, assert on wire behavior and scene state" rig (M0 scope) is exactly `run_client` + `run_dispatch` + a scene assertion. Reuse the in-process `socketpair` + `wayland-client` pattern: deterministic, no external sockets, no external processes, CI-friendly.
3. **Use the default pure-Rust `wayland-backend`** (no `*_system` feature). Proven to build and run with no `libwayland` present — keeps headless CI free of a C Wayland stack.
4. **Build the §7 thread skeleton at `shards = 1`** now (T-proto[1] owning a `Display`, publishing `SceneMsg`-equivalents to the scene owner), so M1's "thread skeleton per §7 even if protocol shards = 1" is satisfied and the `ProtocolHost` seam exists from the start.
5. **Land the Q1 static `Send`/`Sync` assertions as a real test** in `crates/core` when the protocol layer arrives — an invariant (the sharding interface's soundness) without a test is a wish (CLAUDE.md), and this is the regression guard against a future Smithay/`wayland-server` bump quietly removing `Display: Send`.

---

## 6. Threats to validity / what this spike did *not* prove

- It exercised **one** global (`wl_compositor`) and **one** client. It did not stand up `xdg-shell`, multiple clients, or actual N-shard operation. The Q1 sharding recipe is argued from API + types, not run at N>1. Confidence is high (the types compel it) but it is not an executed N-shard demo — that belongs in M1/M2 if/when shards>1 is pursued.
- It used a **sleep-poll** dispatch loop, not `calloop`. Production uses `calloop` on the socket fd; that's a mechanism swap, not a threading-model question, so it doesn't affect the verdict.
- Renderer/backend claims (Q3, M2 layers) are from feature/module inspection and the CHANGELOG, not from building against real DRM hardware — consistent with CLAUDE.md's "verify on real hardware" rule, which lands in M2, not here.

---

## 7. Reproduction

```
cd tools/spikes/smithay-threading
cargo run          # prints Q1 static-fact line + Q2 split trace, exits 0 on PASS
```

The crate is its own workspace and is excluded from `make test`. Version evidence is frozen in its committed `Cargo.lock`.

---

## 8. Drafted decision-log entry — NOT YET APPENDED

> Roland: read the report, confirm or amend, then append this to
> `docs/parhelion_decision_log.md` under a `## 2026-07-24 — Smithay threading fit`
> heading and strike the matching "Pending" item. That append is a small,
> clearly-scoped change (CLAUDE.md) — no separate session needed.

```markdown
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
```
