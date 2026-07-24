# SPINE `desktop` Dialect — Parhelion Control Plane

**Companion to:** `VISION.md`, `CORE-BOUNDARY.md` (§9), vendored
`spine_core_v0_4_design.md`, `spine_dialect_template.md`
**Project:** Parhelion (dialect owned here, not by ENO)
**Status:** stub v0.1 — thin-slice scope, ENO T0 discipline: commit only
what the first running interpreter will execute; reserve the rest.
**SPINE core pin:** v0.4 (vendored copy is authoritative for Parhelion)

---

## 0. What this document is

Parhelion's control plane — the privileged protocol by which policy
daemons, shell components, and extensions steer the compositor — is a
SPINE dialect. Daemons submit SPINE fragments in the `desktop` dialect;
the core's interpolation engine (CORE-BOUNDARY C7) is a dialect
interpreter in the sense of the SPINE dialect contract, a sibling of
ENO's NERVE. No code is shared with NERVE; the *language* is.

This satisfies invariant I-6 (declarative control plane) by
construction: a SPINE fragment is a description; the core executes it
on its own clock. It satisfies I-12 (ship intent) because SPINE is
intent-shipping made rigorous — "structural compression before entropy
coding" and "ship intent, not results" are the same principle.

Following ENO's stub discipline: v0.1 commits the minimum entity set
for the first thin slice (a policy daemon animating window geometry
with springs and tweens, plus one pointer-driven signal). Everything
else is reserved namespace.

### 0.1 Decoupling contract with ENO

- Parhelion **vendors** the SPINE core spec (v0.4) under
  `third_party/spine/`. That copy is authoritative for Parhelion.
- ENO evolves SPINE freely. Core changes reach Parhelion only by
  **deliberate import** (a decision-log entry citing the ENO version
  imported and the migration performed). Never automatically.
- The `desktop` dialect is defined and versioned **here**. ENO is not
  obligated to know it exists.
- NERVE and Parhelion's C7 interpreter are **independent
  implementations** of the dialect-interpreter contract. No shared
  runtime crates in v0.1. (A shared parser crate for the text/binary
  forms may be extracted later if divergence pain exceeds coupling
  pain; that requires a decision-log entry in both projects.)
- Submissions declare `spine_version` and `dialects: { desktop: "…" }`
  exactly as `.spine5` files do. The core rejects fragments pinned to
  versions it does not implement — cleanly, at submit time.

---

## 1. Domain

- **Domain name:** `desktop`
- **Purpose:** Describe *what should happen* to scene-graph state over
  time — animations, transitions, continuous couplings — so that the
  core can execute *how*, per frame, on its realtime-scheduled clock,
  with no callback into any external process.
- **Status:** stub v0.1.

## 2. Type ids (committed for v0.1)

### `desktop.prop` — the sink family

```text
Type id:         desktop.prop
Kind:            primitive
Description:     A scene-graph property endpoint (sink). The only
                 legal LNK destinations in this dialect.
Lifetime:        sink
Required params: surface : ref      (a surface handle the submitting
                                     client's security context is
                                     granted; see §8)
                 property : symbol  (x, y, width, height, opacity,
                                     scale, rotation_z; v0.1 set)
Notes:           Sinks are the capability surface. The core validates
                 at submit time that the fragment's security context
                 holds a grant for (surface, property). Properties
                 outside the v0.1 set are rejected, not ignored.
```

### `desktop.tween`

```text
Type id:         desktop.tween
Kind:            primitive
Description:     Interpolate a property from its CURRENT value to a
                 target over a duration along a curve.
Lifetime:        event-driven (fires on activation; streams only for
                 its duration, then self-terminates)
Required params: target_value : f32
                 dur          : f32 : samples? → ms   (see §6)
Optional params: curve : curve = ease_out_cubic
Ports:           out — signal (the interpolated value)
                 done — event (fires at completion)
Notes:           "From current value" is load-bearing: tweens compose
                 with interruption (§7). There is no from_value param
                 in v0.1; absolute-from tweens haven't earned a use
                 case.
```

### `desktop.spring`

```text
Type id:         desktop.spring
Kind:            primitive
Description:     Damped spring driving a value toward a retargetable
                 setpoint from its current value and velocity.
Lifetime:        streaming (ticks every frame while active; the core
                 auto-terminates it on settle, see §7)
Required params: stiffness : f32
                 damping   : f32
Optional params: initial_target : f32
                 settle_epsilon : f32 = (per-property default)
Ports:           target — signal or value (retarget input)
                 out    — signal
                 settled — event
Notes:           The desktop's favorite primitive. Springs are why
                 the IR is retargetable rather than timeline-bound:
                 a window drag releases into a spring whose target
                 the policy daemon may re-SET at any time.
```

### `desktop.signal.pointer`

```text
Type id:         desktop.signal.pointer
Kind:            primitive
Description:     Continuous pointer-derived source.
Lifetime:        streaming (zero-cost when nothing consumes it)
Required params: field : symbol   (x, y, vx, vy)
Optional params: space : symbol = output   (output | surface)
Ports:           out — signal
Notes:           Reading pointer position is a capability (§8): input
                 signals are granted per security context, exactly as
                 sinks are. This is how "parallax wallpaper" is legal
                 for the wallpaper client and keylogging-adjacent
                 telemetry is not legal for anyone else.
```

### `desktop.gain`

```text
Type id:         desktop.gain
Kind:            primitive
Description:     value = in * scale + offset. The minimum plumbing
                 needed to make signal routing useful in v0.1.
Lifetime:        streaming
Required params: scale : f32
Optional params: offset : f32 = 0.0
Ports:           in — signal   out — signal
```

### Reserved namespace (claimed, not specified)

```text
desktop.transition.*     compound library: window_open, window_close,
                         workspace_switch, genie… (live in the policy
                         daemon's dialect library, ship as compounds)
desktop.signal.audio.*   PipeWire-derived signals (band energy, f0)
desktop.signal.surface.* per-surface signals (content size, damage rate)
desktop.gesture.*        recognizers emitting events
desktop.curve.*          named curve constants beyond the builtin set
desktop.decor.*          decoration descriptions (rounded-rect, shadow,
                         gradient params — per ADR/decision log entry on
                         procedural content: compositor-owned vocabulary)
desktop.shape.*          declared outline paths (the shape extension's
                         dialect face)
desktop.layout.*         layout-result submission from policy daemons
```

Do not invent entries before a running score fragment needs them
(ENO rule, adopted verbatim).

## 3. Ports — shapes and thread transport

Port shapes carry SPINE v0.4 semantics (signal / value / event) plus a
Parhelion commitment that resolves NERVE's open question §9.7 from the
compositor side, formalizing the continuous case:

```text
signal  — continuously varying value. Transport: lock-free SPSC,
          latest-value-wins, written by the producing thread, read by
          the consuming thread once per frame. Implies per-frame cost;
          counts against the streaming budget (§9).
value   — sampled on demand at wiring evaluation. No standing cost.
event   — discrete, queued, lossless delivery. Cost on occurrence.
```

Shape mismatches in LNK are submit-time errors (SPINE rule), which in
Parhelion doubles as admission control: nothing malformed reaches the
frame path.

## 4. Operators (v0.1)

```text
stretch <factor>       tween/compound: multiply all durations
delay <ms>             any: offset activation relative to anchor
curve <curve>          tween: substitute the interpolation curve
stiffen <factor>       spring: scale stiffness (and damping per the
                       critical-damping-preserving rule; see notes)
```

MOD chains and verb forms per SPINE core. Everything else deferred.

## 5. Override keys

`stretch`, `delay`, `curve`, `stiffness`, `damping`, `target_value`,
`dur` — the operators above plus direct parameter replacement, mirroring
ENO's convention that exact-value replacement is a USE override while
proportional adjustment is a MOD.

## 6. Time interpretation — the event-anchored extension

SPINE's `at` is production-timeline time; a desktop has no production
timeline. The `desktop` dialect therefore interprets USE time as
**anchor-relative**:

```text
USE <entity> [anchor <event>] [at <offset>] [dur <d>] { overrides }
```

- **anchor** names an event source: `submit` (default — the moment the
  fragment is accepted), a core-emitted lifecycle event on a granted
  surface (`map`, `unmap`, `focus_gained`, `focus_lost`,
  `grab_released`), or an event port of another entity in the fragment
  (`my_tween.done` — this is how sequences chain).
- **at** offsets from the anchor. Default 0.
- **dur** for tweens is the tween duration; springs have no dur — they
  run to `settled` (or retraction). Open-ended lifetimes are native
  here, unlike a demo timeline.
- GRP local time still exists and still composes: a
  `desktop.transition.*` compound uses GRP-relative offsets internally
  exactly as a cello gesture does. Only the *outermost* anchoring
  differs from ENO usage.

This extension is dialect-level: it defines what `at` means for
`desktop` types, exactly as each ENO dialect declares its own time
interpretation (template §1.6). SPINE core is untouched.

## 7. Liveness: submit, amend, retract, interrupt

The offline pipeline (expander → PICKY → SMOLR) does not exist here.
Its duties move to **submit time in the core**:

1. **Submit.** A daemon sends a fragment (a set of DEF/USE/MOD/LNK/GRP
   statements). The core resolves references, validates shapes,
   enforces opacity, checks capabilities (§8) and budget (§9), then
   activates. Rejection is atomic — a fragment is admitted whole or
   not at all, with a diagnostic.
2. **Amend.** SET on a granted parameter of a live instance (e.g.
   re-targeting a spring via its `target` port or a SET on
   `initial_target`). Applied at the next frame boundary.
3. **Retract.** Revokes a USE (or a whole GRP) by handle. Semantics:
   streaming entities stop at the next frame boundary; the affected
   properties HOLD their current values (no snap-back). A retraction
   may carry `then <entity>` to hand off — the idiom for "cancel the
   fling, spring home."
4. **Interrupt.** New wiring targeting a property already driven by a
   live entity: the previous driver is auto-retracted with HOLD
   semantics, and the new entity starts *from the held current value
   and velocity* (tweens and springs both read current state by
   design, §2). Per-property last-writer-wins, within capability
   grants. Two *different* daemons fighting over one property is a
   grants-design smell, not a runtime feature.
5. **Resync** (invariant I-5). On reconnect, a daemon receives the
   canonical scene state and the list of its own surviving live
   fragments (by handle), then re-submits or retracts as it sees fit.
   Daemon restart is the ordinary path.

## 8. Security: opacity as capability

SPINE v0.4's opacity rule — external wiring may target only declared
ports and TAPs, never subgraph internals — is enforced here as a
capability boundary, not just an encapsulation nicety:

- Every fragment carries its client's security context
  (`wp_security_context_v1` lineage; CORE-BOUNDARY I-7).
- **Sinks** (`desktop.prop`) require a per-(surface, property) grant.
- **Sources** (`desktop.signal.*`) require per-signal grants — reading
  input is as gated as writing geometry.
- Compound internals submitted by one client are opaque to every other
  client *and* to less-privileged wiring by the same client: TAPs are
  the only cross-fragment wiring points, and TAP exposure is itself
  checked against grants.
- The reference policy daemon holds broad geometry grants; a wallpaper
  holds `signal.pointer` + its own surface's props; an ordinary app
  holds nothing in this dialect.

## 9. Admission budget

Adopted from NERVE's frame-budget concept (`nerve_runtime_model.md`
§7), moved from build-time warning to runtime admission:

- Every streaming-lifetime type declares a per-frame cost class in
  this spec.
- The core tracks the active streaming set and its summed cost per
  output. A submission that would exceed the output's budget is
  rejected with a diagnostic (or admitted with degraded frame-rate
  intent flags, once such flags exist — deferred).
- Corollary (invariant I-9): an output whose active streaming set is
  **empty** is eligible for collapse to the damage-tracked 2.5D
  regime. Regime detection is bookkeeping the IR already does.

## 10. Wire form (v0.1: practical, revisable)

- Transport: the private control-plane socket (CORE-BOUNDARY §9).
- Encoding: **JSON** fragments matching the SPINE v0.4 JSON5 data
  model (daemons may author in JSON5; the submitting library strips to
  JSON — comments are for files, not sockets). serde on both ends.
- The SPINE **binary** form (`.spnb` framing) is explicitly *not*
  adopted for the control plane in v0.1: fragments are hundreds of
  bytes on a local socket; 64k discipline is ENO's constraint, not
  Parhelion's. Revisit only if profiling shows submit-path cost, or if
  ENO's binary toolchain stabilizes enough that one shared parser
  beats two simple ones (decision-log entry required, both projects).
- Version pinning per §0.1: fragments declare `spine_version` and the
  `desktop` dialect version; mismatches are rejected at submit.

## 11. Interpreter notes (C7)

- Implementation: Rust, inside the core, per the dialect-interpreter
  contract (`on_def`, `on_use`, `on_set`, `on_mod`, `on_lnk` with
  source lists, `on_grp_enter/exit`, `on_tap`, `finalize` → but
  `finalize` is per-fragment admission, not whole-program emission).
- Evaluation: per frame, T-render's snapshot assembly asks C7 for the
  current value of every driven property. Springs integrate with the
  frame's dt; tweens sample their curve; signal edges read their SPSC
  latest values. No allocation in the per-frame path; instance state
  lives in per-fragment arenas sized at admission (NERVE's allocation
  strategy §8, adopted).
- Determinism: given identical submissions, anchors, dt sequence, and
  seeds, evaluation is byte-identical (ENO's reproducibility rule).
  This is what makes animations golden-testable in the headless
  harness — record a dt trace, replay, compare snapshots.

## 12. Open questions

1. Curve vocabulary for `: curve` params — adopt ENO's tag+payload
   encoding table verbatim, or subset it? (Lean: adopt verbatim; one
   less divergence.)
2. Per-property velocity bookkeeping: which properties carry velocity
   state for interruption handoff (geometry certainly; opacity?).
3. `desktop.transition.*` compound library ownership: reference
   compounds ship with the policy daemon or with the core's dialect
   library? (Lean: daemon — the core ships mechanisms, not taste.)
4. Multi-output anchoring: what does a workspace-switch compound
   anchored on two outputs mean when the outputs run different
   refresh rates?
5. Cost-class units for §9 (NERVE open §7's question, shared):
   microseconds-per-tick with per-machine calibration is the current
   lean.
6. Signal-shape LNK formalization should be mirrored into ENO
   (`continuous_lnk` answer to `nerve_runtime_model.md` §9.7) — an
   ENO-side decision; flagged, not assumed.
