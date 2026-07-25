# M2 Task Slicing — "On the metal"

> **Re-entrancy header.**
> **Status:** v1.0 · **Date:** 2026-07-25 · **Kind:** milestone task plan (`docs/plans/`).
> **Upstream:** `docs/parhelion_milestone_plan.md` M2; CORE-BOUNDARY §7 (T-input, T-commit), I-2, I-11; the M1 debt ledger (each item cited at its task).
> **Goal restated:** boots from a TTY on real hardware and is cautiously daily-drivable: DRM/KMS atomic, libinput on a real T-input thread, hardware cursor plane, GPU renderer with dmabuf + explicit sync, frame scheduler, presentation-time, VT survival.

Ordering note: T1–T3 make the metal real with the existing CPU
renderer (dumb buffers) — each stage bootable and verifiable. The GPU
arrives T4–T6 behind a spike, because renderer API choice is a
decision, not a default. Debt tasks bracket the milestone: T0 pays
M1's promissory notes; T7 retires the subcompositor tripwire.

---

## T0 — Debt & honesty *(prompt 12)*

The M1 ledger, paid before new debt is taken on:
- **Subcompositor tripwire** (per the superseded/refined decision
  conveyed in prompt 12): `get_subsurface` posts a protocol error
  with a clear message; advertised global stays; rig test asserts the
  loud refusal; map part `dnd`-style seam for subsurfaces → T7.
- **fd-deregistration backpressure** (T2's recorded M2 promise): a
  throttled client's socket source is deregistered from the loop and
  re-registered on drain — ending the dispatch-thread spin during an
  active flood. The T2 flooding test grows a no-spin assertion
  (dispatch loop iterations bounded during flood, via counter).
- **Decision-log entries:** the refined advertise/refuse principle
  (superseding entry); anything the fd work makes load-bearing.
**Acceptance:** loud-refusal test; flood test with spin counter;
entries landed; all prior tests green.

## T1 — Session, DRM/KMS atomic, dumb buffers *(prompt 13)*

`crates/backend-drm`: libseat session (seatd/logind), DRM master,
connector/mode selection (single output, preferred mode), atomic
commits presenting the existing CPU-rendered frames via dumb buffers,
VT switch away/back with clean reacquisition. The §7 **T-commit**
thread is born here — atomic submission and vblank events on their
own thread, per CORE-BOUNDARY. No GPU, no planes beyond primary yet.
**Acceptance:** boots from a TTY on the dev machine showing core-owned
content + a foot session; VT round-trip test (scripted where possible,
checklisted where not); T-commit ownership visible in code; headless
CI untouched (DRM code behind its crate boundary, unit-tested logic
extracted where testable).

## T2 — libinput, the real T-input, cursor plane *(prompt 14)*

The §7 deviation retired: a dedicated T-input thread owning libinput,
producing the existing `InputEvent` funnel (the M1 interface meets its
intended producer — winit path remains for nested dev). Hardware
cursor plane driven from T-input via atomic cursor updates — I-2 made
literal: cursor motion bypasses the render path entirely. Client
`set_cursor` honoured (cursor surfaces composited to the cursor
plane; hidden/default handling). The map's `t-input` seam node fills;
the scene doc's deviation section gains its "resolved in M2" coda.
**Acceptance:** cursor latency instrumented and unaffected by an
artificially slowed render thread (the milestone's I-2 test, now on
metal); funnel unchanged (rig suite green untouched); keymap/keycode
path identical between producers (one shared test).

## T3 — Frame scheduler, presentation-time, callback pacing *(prompt 15)*

Render-as-late-as-possible: measured render time + margin against the
vblank deadline (per-output), `presentation-time` protocol with real
flags (vsync, hw clock), and frame callbacks re-paced to presentation
(retiring M1's fire-every-tick v1 — its doc note comes due).
Occlusion-aware callback throttling lands here (scene knows extents,
stacking, opaque flags: fully-occluded surfaces get throttled
callbacks, spec-honest). The T2/T7 acceptance tests re-anchor to the
new pacing.
**Acceptance:** scheduler slack instrumented (start-time vs deadline
histogram counters); presentation feedback conformance tests; foot
acceptance test green under new pacing; occluded-surface throttle
test.

## T4 — Spike: renderer API *(prompt 16)*

Vulkan vs GLES for renderer v2, decided on evidence: explicit-sync
integration (I-11 wants syncobj-native), dmabuf import ergonomics,
software-rasterizer CI story (lavapipe vs llvmpipe — golden testing
must survive), tolerance policy the GPU forces on the golden rig,
Smithay ecosystem friction for whichever we pick (protocol-side only,
per the seam), and the ADR-0002 horizon (SPIR-V as the open
vocabulary favors Vulkan — weight, not verdict). Deliverable: report +
drafted decision entry, Roland confirms — the prompt-01 pattern.
**Acceptance:** report with runnable evidence; recommendation stated
for a one-sentence yes/no; no production code.

## T5 — GPU renderer v1 *(prompt 17)*

The chosen API renders the scene: textured quads, integer-equivalent
blending, damage-scissored partial renders on the retained
swapchain image where the API allows. CI runs it on the software
rasterizer; goldens migrate under the per-test tolerance rule
(harness_design.md's policy earns its keep — every loosened
tolerance states its reason). CPU renderer remains for headless
determinism tests and as the reference for an incremental-equals-
scratch cross-check (GPU vs CPU within tolerance).
**Acceptance:** golden suite green on lavapipe/llvmpipe in CI;
equivalence property re-proven on GPU; dumb-buffer path replaced on
metal; damage proportionality counters still meaningful (GPU
pixels-shaded proxy documented).

## T6 — dmabuf import & explicit sync *(prompt 18)*

`linux-dmabuf-v1` with real format/modifier negotiation from the
renderer's capabilities; `linux-drm-syncobj-v1` as the primary sync
path (I-11), implicit-sync shim documented as compatibility; the
texture-source seam gains its second real member — and the module-doc
sentence ("nothing may assume pixels are locally produced") gets its
first GPU-side test. This is also the Rayland allocator-namespace
groundwork the milestone plan pulls forward: the import path is
written so a token-resolved dmabuf is indistinguishable from a
client-provided one (M6 fills the resolver).
**Acceptance:** a GPU client (e.g. `weston-simple-egl` class) renders
through Parhelion on metal; syncobj wait/signal exercised in tests
where the CI GPU allows, gracefully skipped with a loud note where
not; format negotiation conformance tests.

## T7 — Subsurfaces v1 *(prompt 19)*

The real fix retiring T0's tripwire: scene child nodes with relative
position, sync/desync commit semantics, stacking above/below parent,
input hit-testing through the tree. The advertise/refuse entry gets
its closing coda; the tripwire test inverts (get_subsurface now
succeeds; a subsurface composites).
**Acceptance:** conformance tests for sync/desync and placement;
golden with a subsurface overlapping its parent; a real subsurface
client (weston-subsurfaces or scripted) runs; tripwire retired.

## T8 — M2 acceptance run & closure *(prompt 20)*

Full session on the dev machine from TTY: foot + a GPU client,
cursor under artificial render load (I-2 demonstrated live),
VT round-trip, suspend/resume if cheap (else M9). The
hostile-slow-render CI test from the milestone acceptance
("no protocol client can stall a commit deadline in the M-scoped
stress test") formalized. Item-by-item walk; status line; map.
**Acceptance:** the milestone plan's M2 list, every item green with
evidence, or the honest blocker report.

---

## Standing notes for M2

- **Hardware honesty:** driver behavior observed on the dev machine
  that contradicts documentation goes in the diary (CLAUDE.md's
  technical-attitude rule was written for this milestone).
- **The winit backend stays** as the nested dev path; CI stays
  headless. Every task states what is CI-verifiable vs. dev-machine
  vs. Roland's eyes — the T6/T7 pattern.
- **Goldens under GPU:** tolerance loosening is per-test with stated
  reason, never global; the CPU renderer remains the determinism
  anchor.
- **No plane offload beyond cursor** (M5 owns overlay/scanout
  demotion); no HDR/color (M9 executes the deferred decision); no
  multi-output (M9).
