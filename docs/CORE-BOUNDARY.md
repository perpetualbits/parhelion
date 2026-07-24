# Parhelion — Core Boundary Specification

> **Re-entrancy header** — read this block to reconstitute context.
> **Status:** Draft v0.1 · **Date:** 2026-07-23 · **Kind:** P2 — normative specification. Governs process placement, invariants, threading, and failure semantics for everything in the Parhelion system.
> **Upstream:** `VISION.md` (especially Theses 2–5). **Downstream:** all crate-level designs, all Claude Code task descriptions.
> **Normative language:** MUST / MUST NOT / SHOULD as in RFC 2119. Invariants are numbered **I-n** and referenced by number in code review, tests, and `CLAUDE.md`.
> **Change control:** any change to §3 (core contents), §5 (invariants), or §6 (process inventory) requires a decision-log entry.

---

## 1. Purpose

This document draws the single most important line in the system: **what runs inside the core process, and what is exiled to a server process.** It exists so that this question is never re-litigated ad hoc — in a code review, in a Claude Code session, or at 2 a.m. when in-process would be easier.

## 2. Definitions

- **Core** — the one process whose death ends the session (modulo §8 restart survivability). Realtime-capable, minimal, owns the hardware-facing frame path.
- **Frame path** — the code executed between "decision to produce a frame" and "atomic KMS commit submitted," plus input delivery from evdev to client socket. The hot path that invariants I-1..I-4 protect.
- **Server** — a Parhelion-supplied separate process providing a system service (policy, replay, effects, portals). QNX vocabulary, on purpose.
- **Client** — any process speaking Wayland to the core, including all servers that render anything.
- **Control plane** — the set of privileged protocols by which servers observe and steer the core (placement, focus policy, animation submission). Declarative by construction (I-6).
- **Canonical state** — the core-owned truth: surface tree, geometry, stacking, focus, capability grants, output configuration.
- **Supervisor** — a minimal separate process (init-like) that spawns, monitors, and restarts core and servers per §8.

## 3. What is IN the core

Exhaustive. Anything not listed here is outside (§4). Additions require a decision-log entry.

| # | Component | Why it cannot leave |
|---|-----------|---------------------|
| C1 | DRM/KMS backend: atomic commits, plane assignment, mode setting, hardware cursor | Frame-path deadline; privilege (DRM master) |
| C2 | Input pipeline: libinput/evdev → routing → focused-client delivery; cursor motion → cursor plane | Latency; must never wait on rendering |
| C3 | Wayland protocol machinery (core + supported extensions), per-client dispatch | Ordering guarantees; canonical-state ownership |
| C4 | Scene graph: canonical state, node tree, transforms, texture-source bindings | It *is* the canonical state (I-5) |
| C5 | Render loop: snapshot → damage/regime decision → record → submit; frame scheduler (render-as-late-as-possible) | Frame-path deadline |
| C6 | Regime state machine: 2.5D damage-tracked ↔ 3D full-frame, per output; plane-offload decisions | Inseparable from C1/C5 |
| C7 | Interpolation engine: executes declarative animation programs (curves, springs, timelines) submitted via the control plane | The "how" half of ship-intent; must run on the core's clock |
| C8 | Capability enforcement: `wp_security_context_v1` tagging, per-client grant checks on every privileged request | Enforcement must sit with canonical state; a server cannot police itself |
| C9 | Buffer import: dmabuf, shm, **token-buffer** (Rayland S-side); explicit sync (`linux-drm-syncobj-v1`) wait/signal | Frame path; GPU allocator namespace shared with R1 by design |
| C10 | Minimal built-in fallbacks: solid-color decorations, default window placement, "server crashed" surface | Crash-only requirement: the core must remain usable with every server dead |

**Explicitly NOT in the core** (non-exhaustive, for emphasis): window-management *policy*, tiling logic, animation *decisions*, panels/launchers/notifications/lock screen, screenshot/screencast/portals, configuration parsing, the Rayland replay service, XWayland, effect shaders beyond composition, any third-party code of any kind.

## 4. Placement rules (the algorithm)

For any proposed component, apply in order; first match wins:

1. **Third-party code?** → Server (or WASM extension host), no exceptions, ever. *(I-4)*
2. **Parses hostile or remote input?** (Rayland command stream, network protocols, complex file formats) → Server, sandboxed (seccomp, own render node where GPU-touching, quotas). *(I-8)*
3. **Can it block, sleep, do I/O, or take unbounded time?** → Server, or restructure so the blocking part is a server and only a declarative result crosses into the core.
4. **Is it policy?** (a decision that a reasonable user might want different) → Server on the control plane.
5. **Is it on the frame path or does it own hardware privilege?** → Core.
6. **Otherwise** → default to Server; in-core placement requires a decision-log entry arguing timing necessity.

Memory safety is *not* a criterion: Rust provides it in-process. Process boundaries here buy **fault**, **timing**, and **privilege** isolation only (VISION Thesis 2).

## 5. Invariants

Each invariant names its enforcement mechanism. An invariant without a test is a wish.

- **I-1 — The frame path MUST NOT block.** No syscalls that can sleep indefinitely, no locks shared with non-frame threads, no synchronous IPC, no allocation in the commit hot loop where avoidable. *Enforced by:* code review checklist + a CI stress test with hostile servers (VISION §6.2) + `#[deny]`-style lint list in `CLAUDE.md`.
- **I-2 — Input delivery MUST NOT wait on rendering.** Cursor motion reaches the cursor plane and events reach the focused client regardless of render-thread state. *Enforced by:* thread ownership rules (§7) + latency instrumentation.
- **I-3 — The core MUST NOT make synchronous calls into any server.** All server communication is asynchronous message passing; the core proceeds with fallbacks (C10) when answers are late. Late answers apply next frame or never.
- **I-4 — Third-party code MUST NOT execute in the core process.** Structurally: the core links no plugin loader, embeds no interpreter, dlopens nothing.
- **I-5 — Canonical state lives in the core; servers hold only derived, reconstructible state.** Every control-plane protocol MUST include a full-resync operation, and server restart MUST be implemented as: connect → resync → resume. *Enforced by:* the crash-only CI test (kill each server during interaction).
- **I-6 — The control plane is declarative.** Servers submit descriptions (target states, animation programs, layout results); the core executes them on its own clock. There is no per-frame callback into any external process. Escape hatch: the dmabuf filter path (§6, E1), which the core MAY skip on any frame that would otherwise miss deadline — degrade, never stall.
- **I-7 — Capability checks are enforced in the core at request time.** No privileged operation (screencopy, input injection, layer-shell, control plane, token-buffer attach) proceeds without a grant attached to the client's security context. Remote (Rayland) clients default to a strictly smaller grant set than local clients.
- **I-8 — Hostile-input parsers are sandboxed servers.** The Rayland replay service runs seccomp-confined, on its own DRM render node, with VRAM quota, command-rate limit, and GPU-reset watchdog; the core touches only the resulting dmabuf token and syncobj. (Inherits Rayland design §8 verbatim.)
- **I-9 — Regime collapse is mandatory.** After the last active animation/3D interaction on an output, that output MUST return to damage-tracked, plane-offloaded operation within 2 frames. *Enforced by:* instrumentation counters + CI scenario test.
- **I-10 — Every byte of GPU work is attributable.** Per-client (including per-server) accounting of VRAM and GPU-time, so a misbehaving client is identifiable and quota-limitable. Prerequisite for I-8's quotas and for debugging the founding grievance ("what is stuttering my desktop?").
- **I-11 — Explicit sync is the primary path.** `linux-drm-syncobj-v1` throughout; implicit sync is a compatibility shim. (Required by Rayland's remotable-sync design; also the modern driver reality.)
- **I-12 — Ship intent across every boundary.** Any protocol, internal or external, that transports results (pixels, per-frame positions) where intent (descriptors, programs) would suffice requires a decision-log entry documenting why. (VISION §2.)

## 6. Process inventory (initial)

| ID | Process | Trust level | Restart policy | Talks via |
|----|---------|-------------|----------------|-----------|
| P0 | Supervisor | root of trust | respawns others; if core dies, restarts it (§8) | spawn/waitpid, control socket |
| P1 | **Core** | full | restart = session recovery path (§8) | — |
| S1 | Policy daemon (window management: placement, tiling, focus rules) | control plane | kill-and-resync, unlimited | private control-plane protocol |
| S2 | Shell clients: panel, launcher, wallpaper, notifications, lock screen* | layer-shell grants | ordinary clients; crash is cosmetic | Wayland + layer-shell |
| S3 | Config service (parses config, feeds S1/S2, submits core settings declaratively) | control plane (settings subset) | kill-and-resync | control plane |
| R1 | Rayland replay service (one per remote session) | sandboxed (I-8) | kill; remote client reattaches via its proxy | token-buffer + syncobj + allocator namespace |
| R2 | Rayland session broker (auth, QUIC endpoints, spawns R1) | network-facing, sandboxed | kill-and-resync | control plane (session grants) |
| E1 | Effect renderer(s): out-of-process dmabuf filters (blur, exotic effects) | GPU, unprivileged | kill; core renders unfiltered | dmabuf in/out + syncobj |
| X1 | XWayland + bridge | legacy compat, confined grants | kill; X apps die (acceptable) | Wayland |
| W1 | Extension host: WASM components, fuel/epoch-preempted, capability-scoped imports | per-extension grants | kill/suspend per extension | control plane (scoped) |
| U1 | Portal/screenshot/screencast service | explicit user-granted capabilities | kill-and-resync | Wayland ext + portals D-Bus |

\* Lock screen requires special care: its *enforcement* (input capture, blanking policy) is core capability logic; only its *rendering* is S2. An unlockable-but-crashed lock UI must fail *locked*. → decision-log entry (with reasoning in a design doc) required before implementation.

## 7. Threading model inside the core

Ownership is absolute: each resource has exactly one owning thread; cross-thread communication is message passing over bounded channels; the scene graph crosses threads only as immutable per-frame snapshots.

- **T-input** — evdev/libinput, routing, cursor-plane updates. SCHED_FIFO candidate. Owns: input devices, focus routing table (read-mostly replica of canonical focus).
- **T-commit** — atomic KMS commits, vblank handling, frame scheduling deadlines. SCHED_FIFO/SCHED_DEADLINE candidate. Owns: DRM fd, plane state.
- **T-render** — owns the GPU context and the render graph; consumes scene snapshots; records and submits. Not RT (GPU is not preemptible; RT here is dishonest).
- **T-proto[n]** — sharded Wayland client dispatch. Per-client ordering preserved within a shard; cross-client independence exploited across shards. Owns: client sockets, protocol object state; publishes state changes to the scene graph via messages.
- **T-workers** — pool: damage region algebra, texture upload (transfer queue), mip/mask generation, snapshot assembly.

RT honesty (from VISION): SCHED_FIFO on T-input/T-commit bounds *CPU-side* jitter — "never miss the commit deadline because the CPU was busy." It does not and cannot make the GPU realtime. The frame scheduler renders as late as measured render time + margin allows; tearing-control is supported for the cases that want it.

## 8. Failure & restart semantics

- **Any S/E/W/U/R process dies** → supervisor restarts it (rate-limited); core continues with C10 fallbacks; on reconnect the process resyncs (I-5). User-visible effect: at most a cosmetic flicker of that component. This is the *ordinary* path and is exercised in CI.
- **R1 dies or hangs** → GPU-reset watchdog fires if needed; remote client's proxy reattaches through R2; core never noticed beyond a stale token.
- **Core dies** → supervisor restarts the core. Local ordinary clients die (Wayland reality). **Loopback-proxied clients and Rayland clients survive:** their proxies hold reattachment state (VISION Thesis 6). Restored session state (output config, workspace layout) comes from the core's periodic canonical-state journal; window-geometry restoration for restarting local apps rides the emerging session-management protocol. Target: core restart is a development-workflow feature, not only a disaster path.
- **Supervisor dies** → session over; keep P0 too small to have bugs (target: small enough to read in one sitting).

## 9. Control-plane sketch (normative constraints only; full spec is its own document)

- Event stream core→server: canonical-state deltas + full resync on demand (I-5).
- Command stream server→core: declarative only (I-6) — target geometries, stacking constraints, animation programs (curve/spring/timeline IR executed by C7), grant requests.
- Every message carries the submitting client's security context; the core enforces (I-7).
- Versioned; a server built against an older control plane keeps working or fails cleanly at connect.

## 10. Open questions (each becomes a decision-log entry when settled)

1. Lock-screen fail-locked design (§6 note).
2. Animation IR expressiveness: exact curve/spring/timeline vocabulary of C7; what is deliberately inexpressible.
3. Scene-snapshot representation: full copy vs. persistent (structural-sharing) tree; budget for snapshot assembly.
4. Smithay fit: which Smithay layers we consume vs. bypass given our threading model (Smithay leans calloop/single-thread).
5. Token-buffer protocol: private first, propose upstream as `linux-dmabuf-v1` sibling later (per Rayland doc §5③).
6. Multi-GPU: render-node selection for R1/E1, cross-GPU dmabuf.
7. Color management / HDR: working-space choice in C5, per-output tone mapping — design in from M0 or M1?
8. WASM host (W1): component model, fuel vs. epoch preemption, capability import surface.
9. Canonical-state journal format and cadence for §8 core-restart recovery.
