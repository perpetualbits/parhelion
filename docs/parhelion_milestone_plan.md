# Parhelion — Milestone Plan

> **Re-entrancy header.**
> **Status:** v0.1 · **Date:** 2026-07-24 · **Kind:** P3 — sequencing document.
> **Upstream:** `VISION.md` (§6 success criteria, §8 "milestones are usable compositors"), `CORE-BOUNDARY.md` (invariants cited by number), `parhelion_desktop_dialect.md`.
> **How to read:** each milestone states Goal, Scope, and Acceptance. Acceptance criteria are tests or measurements, not adjectives. Milestones are sliced into Claude-Code-sized tasks at milestone start (a short `docs/plans/mN_tasks.md` per milestone, written then, not now — slicing months ahead is speculation).

---

## Principles

1. **Every milestone is a usable compositor** at its own level of ambition. No big-bang integration phase exists.
2. **The harness precedes the features.** Nothing lands without a headless-verifiable test; the golden-test rig from M0 is the project's velocity multiplier for AI-assisted work.
3. **Invariants are CI, not prose.** The stall test (I-1), crash-only suite (I-5), and regime-collapse counters (I-9) enter CI at the milestone that makes them meaningful and never leave.
4. **Order may flex; acceptance may not.** M5 efficiency work can interleave earlier; M6 is paced by the Rayland project. A milestone is done when its acceptance list is green, not when its code exists.

---

## M0 — Skeleton & harness

**Goal:** a repository that builds, tests itself headlessly, and carries the project's memory system.
**Scope:** repo scaffolding per `CLAUDE.md` layout; docs installed and indexed; cargo workspace with crate skeletons (`core`, `harness`, `dialect`, backends); vendored SPINE core spec under `third_party/spine/`; headless backend rendering a test pattern to memory; golden-screenshot test rig (render → hash/compare with per-pixel tolerance); protocol test rig (spawn a scripted Wayland test client, assert on wire behavior and scene state); CI running build + tests; the Smithay threading investigation spike (can Smithay protocol types be driven from sharded dispatch threads with per-client ordering, or do we consume its backends only?) closed with a decision-log entry.
**Acceptance:** `make test` green locally and in CI; headless golden test passes; a deliberately broken golden test fails (the rig is proven able to fail); Smithay decision logged; project index lists every document.
**Status: complete 2026-07-24** (sessions: `_session_2026-07-24_scaffolding`, `_session_2026-07-24_smithay-spike`, `_session_2026-07-24_headless-golden-ci`, `_session_2026-07-24_protocolhost-rig`). Every acceptance item verified green; the item-by-item walk is in the last session summary. Deliverables: cargo workspace + docs/memory system; headless backend (`test_pattern`); golden rig with a proven-able-to-fail meta-test; `ProtocolHost` (shards = 1) + scene ledger + protocol rig; CI workflow; Smithay threading decision logged.

## M1 — One window, honestly

**Goal:** a real client renders and receives input, headless/nested.
**Scope:** `wl_compositor`, `wl_surface`, `wl_shm`, minimal `xdg-shell` toplevel; `wl_seat` keyboard + pointer delivery to the focused client; scene graph v1 — canonical state owned by core, immutable per-frame snapshots to the render thread (thread skeleton per CORE-BOUNDARY §7 even if protocol shards = 1 for now); damage-tracked partial redraws with instrumentation counters; frame callbacks.
**Acceptance:** weston-terminal (or foot) runs and echoes typed input under the nested/headless backend; damage counters prove partial redraws (typing redraws a region, not the frame); protocol conformance tests for the implemented globals pass; golden tests for stacking and damage.

## M2 — On the metal

**Goal:** boots from a TTY on real hardware; cautiously daily-drivable.
**Scope:** DRM/KMS atomic backend; libinput; hardware cursor plane driven from T-input (I-2); GPU renderer with dmabuf import; explicit sync (`linux-drm-syncobj-v1`) as primary path (I-11); frame scheduler v1 (render-as-late-as-possible with measured render time + margin); `presentation-time`; VT switching and modeset survival.
**Acceptance:** full session on the dev machine from TTY; cursor motion latency instrumented and unaffected by an artificially slowed render thread (I-2 test); no protocol client can stall a commit deadline in the M-scoped stress test; clean VT switch round-trip.

## M3 — Control plane thin slice (SPINE `desktop` dialect v0.1)

**Goal:** the declarative control plane exists and survives its daemon dying.
**Scope:** control socket + JSON wire form (dialect spec §10); C7 interpreter for the five v0.1 types (`prop`, `tween`, `spring`, `gain`, `signal.pointer`) implementing the dialect-interpreter contract; event anchors (`submit`, `map`, `grab_released`, entity `done`/`settled`); submit/amend/retract/interrupt with HOLD semantics (§7); capability checks on sinks and sources (§8) with a static grants file for now; admission budget v1 (§9); resync (I-5); a minimal out-of-process test daemon.
**Acceptance:** **the spring test** — daemon submits a spring fragment, a surface glides; `kill -9` the daemon mid-flight → property HOLDs, no frame missed; daemon restarts, resyncs, retargets the spring to completion. Deterministic dt-trace replay reproduces byte-identical property curves (dialect spec §11). Malformed fragments (bad shape, ungranted sink, budget-busting streaming set) are rejected atomically with diagnostics.

## M4 — Microkernel for real

**Goal:** the process inventory exists; crash-only is demonstrated, not claimed.
**Scope:** supervisor P0 (spawn, monitor, rate-limited restart); reference policy daemon S1 (placement, focus, minimal tiling) speaking the control plane; config service S3; `layer-shell` and a minimal panel + wallpaper as ordinary clients; capability enforcement v1 via `wp_security_context_v1` tagging (I-7) replacing the static grants file; core fallbacks C10 (default placement, solid decorations, crash surface).
**Acceptance:** the crash-only CI suite — `kill -9` each of {S1, S3, panel, wallpaper} during scripted interaction: session continues, component restarts and resyncs, worst user-visible effect is cosmetic (VISION §6.3). The hostile-daemon stall test (busy-loop, sleep(10), crash-loop, memory-hog daemon variants) causes zero missed frame deadlines (VISION §6.2, I-1) — this test enters CI here and never leaves.

## M5 — Efficiency crown jewels

**Goal:** the boring desktop is provably cheap; VISION §6.1 is measured, not hoped.
**Scope:** KMS overlay-plane offload and direct scanout with aggressive demotion; opaque-region occlusion culling; damage forwarded to KMS (`FB_DAMAGE_CLIPS`); GPU-idle verification between damage events; per-client GPU-time and VRAM accounting (I-10) surfaced in a debug tool ("what is stuttering my desktop" — the founding grievance gets its instrument); benchmark harness comparing idle power and input-to-photon latency against sway on identical hardware.
**Acceptance:** fullscreen and topmost-rect clients hit direct scanout (verified via KMS state); benchmark report checked into `docs/` showing parity-within-noise vs sway for the terminal+browser scenario, or a decision-log entry explaining the honest gap and the plan.

## M6 — Rayland hosting (externally paced)

**Goal:** Parhelion becomes the reference S-side.
**Scope:** token-buffer source implemented for real — shared GPU allocator namespace, remotable syncobj wait (the trait exists from M1's renderer abstraction; this fills it); R1 replay-service sandbox skeleton (separate process, seccomp, own render node, VRAM/rate quotas, GPU-reset watchdog — I-8); R2 session-broker stub; opt-in local loopback proxy for restart survivability.
**Acceptance:** a Rayland client (whatever Rayland's own milestone provides — even the demo client) displays through Parhelion with the core touching only token + syncobj (VISION §6.4: no core code path names Rayland); `kill -9` R1 → remote client reattaches via proxy, core unaffected; core restart with a loopback-proxied client → client survives with window state (VISION §6.6). *Pacing note: this milestone's calendar position depends on Rayland; its interface obligations (the trait, the allocator namespace design) are M1/M2 work precisely so this milestone is integration, not invention.*

## M7 — Shaped windows

**Goal:** windows escape the rectangle, cheaply.
**Scope:** the shape extension protocol (client-declared Bézier outline paths, versioned, per decision log 2026-07-24); declare-time derivation of inscribed (occlusion) and bounding (damage/input) polygons; input hit-testing via point-in-path; occlusion culling consuming declared opaque interiors; alpha-contour extraction as compatibility fallback only; `desktop.shape.*` dialect entries ratified from reserve.
**Acceptance:** demo shaped client (something pleasingly non-rectangular); occlusion test — a window fully behind a shaped opaque interior contributes zero render work (verified by counters); input tests on and just outside the curve boundary; damage regions track the declared bound, not the buffer rect.

## M8 — The third dimension

**Goal:** the 3D-native scene graph earns its name; the regime machine works.
**Scope:** 3D transforms on scene nodes; the regime state machine per output (2.5D damage-tracked ↔ 3D full-frame) with I-9 collapse instrumentation; 3D-regime renderer with depth prepass and occlusion culling from declared opaque geometry; cached shadow atlas for static casters; mesh nodes (first true 3D desktop objects); 3D input mapping (raycast against transformed surfaces → surface-local coordinates).
**Acceptance:** a window-rotate animation flips its output to the 3D regime and collapses back within ≤2 frames of settle (I-9 counters in CI); a static 3D ornament costs near-zero per frame once its shadow is cached (measured); typing into a 3D-tilted terminal works; VISION §6.5 green.

## M9 — Citizenship

**Goal:** the daily-driver checklist.
**Scope:** multi-output with mixed refresh rates; color management / HDR (execute the deferred decision — open question 7); XWayland as confined server X1; portals / screenshot / screencast service U1; lock screen (its fail-locked ADR is written *before* implementation, per CORE-BOUNDARY §6 note); WASM extension host W1 with fuel/epoch preemption; session-management protocol for local-app geometry restore.
**Acceptance:** a written daily-driver checklist (browser, terminal, video, screen-share, lock, suspend/resume, hotplug) fully green; W1 hostile-extension test (spinning extension is suspended at budget, session unaffected) joins the CI stall suite.

## Continuous / post-M9

- **The showcase:** a NERVE-class `graphics.visual_field` scene — Desert Monument's undulating field — running as a Parhelion wallpaper via the dialect bridge. Zero architectural weight; maximal morale weight.
- SPINE core version imports from ENO (deliberate, logged, per dialect spec §0.1).
- Upstreaming candidates: shape extension, token-buffer protocol (per Rayland's own plan).
