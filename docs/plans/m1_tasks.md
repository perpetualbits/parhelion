# M1 Task Slicing — "One window, honestly"

> **Re-entrancy header.**
> **Status:** v1.0 · **Date:** 2026-07-25 · **Kind:** milestone task plan (`docs/plans/`).
> **Upstream:** `docs/parhelion_milestone_plan.md` M1; decision log "Smithay threading fit"; spike report §5; chat-project spike review (reverse-direction and backpressure riders).
> **Goal restated:** a real client (foot / weston-terminal) renders and receives input under the nested/headless backend; damage counters prove partial redraws; the §7 thread skeleton exists at shards = 1.

Tasks are ordered by dependency; each is one prompt (prompt number
assigned when authored). A task may span more than one session; a task
is done when its acceptance bullet is green, not when its code exists.

---

## T1 — Scene graph v1, thread skeleton, first composition *(prompt 04)*

Canonical scene state in the core (surface nodes: position, size,
stacking, texture-source binding, opaque flag), owned by a scene thread;
`ProtocolHost` publishes to it by message (the M0 ledger is absorbed —
its tests migrate to scene assertions). Immutable per-frame snapshots to
a T-render skeleton that composites with a CPU renderer v1. Texture
sources are an extensible binding with exactly two members now:
`Solid(color)` for tests, `Shm` arriving in T3 — and a documented
reserved point where dmabuf and Rayland token-buffer sources attach
later (the Rayland interface obligation the milestone plan pulls into
M1). Node model documented as 3D-native with only the axis-aligned path
implemented (Thesis 1/3). New short canonical doc:
`docs/scene_graph_v1.md`; CLAUDE.md table updated.
**Acceptance:** golden tests for stacking/overlap of solid-color nodes;
a snapshot-isolation test (mutating canonical state after snapshot does
not affect the in-flight frame); thread ownership per §7 visible in
code; frame counter instrumentation skeleton exists.

## T2 — Reverse direction: frame callbacks & flush ownership *(prompt 05)*

The spike's unproven half. `wl_surface.frame` callbacks fired from the
render side (scene/render thread decides "frame presented", protocol
thread delivers), flush ownership settled (dispatch thread flushes;
render side only enqueues), and the **backpressure policy** written
into code and `scene_graph_v1.md` (bounded per-client queues or quotas
at the `ProtocolHost` boundary; a flooding client must not stall its
shard-mates — I-10's fairness rider from the spike review).
**Acceptance:** protocol-rig test where a scene-triggered frame
callback reaches the scripted client; a flooding-client test showing
bounded memory and continued service to a second client.

## T3 — wl_shm buffers and commit semantics *(prompt 06)*

`wl_shm` via Smithay's protocol-side machinery — this is the
consume/bypass **seam check** from the spike review: shm protocol
handling without Smithay's renderer traits; report if the seam fights
back. Buffer attach/commit/release lifecycle; committed pixels become a
`Shm` texture source (CPU copy v1, correctness over speed); buffer
release timing correct per protocol.
**Acceptance:** golden test — a scripted client's shm buffer composites
correctly over solid nodes; buffer-release conformance test; the
reserved texture-source seam still clean (no renderer traits imported).

## T4 — Damage tracking v1 *(prompt 07)*

Per-surface damage accumulation, region algebra (surface → scene →
output coordinates), partial redraws honoring the damage region, and
the instrumentation counters the milestone acceptance names
(pixels-redrawn per frame, region counts).
**Acceptance:** counter-verified test: small-damage commit redraws a
proportionally small region; full-damage fallback correct; golden tests
unaffected (damage must not change output, only cost).

## T5 — xdg-shell minimal *(prompt 08)*

`xdg_wm_base` / `xdg_surface` / `xdg_toplevel` lifecycle: configure/ack,
map/unmap, roles, title/app_id captured into scene state; default
placement from core fallback C10 (policy daemon is M4).
**Acceptance:** conformance tests for the configure/ack dance and role
errors; scene reflects map/unmap; a scripted xdg client reaches mapped
state and composites.

## T6 — Seat, input, and the winit nested backend *(prompt 09)*

`wl_seat` with keyboard + pointer; focus = topmost mapped toplevel
(C10 fallback policy, explicitly temporary); `crates/backend-winit`
presenting our CPU frames in a window (softbuffer-class blit — **not**
Smithay's renderer) and feeding winit input events into the input
path. T-input ownership per §7 even though winit forces some
event-loop cohabitation — document the deviation honestly if winit's
main-thread requirement bends the pure model (diary + doc note, not
silent).
**Acceptance:** protocol-rig tests for enter/leave/key/button delivery
and focus follow; interactive smoke: a window Roland can see.

## T7 — M1 acceptance run and closure *(prompt 10)*

The real thing: foot or weston-terminal under the winit backend —
launches, maps, renders, echoes typed input. Damage counters prove
typing redraws a region, not the frame. Conformance sweep for all
implemented globals; item-by-item milestone acceptance walk; status
line in the milestone plan.
**Acceptance:** the milestone plan's M1 acceptance list, every item
stated green, or an honest blocker report.

---

## Standing notes for all M1 tasks

- No Smithay renderer or `desktop`/`space` types anywhere (decision
  log); the seam is watched in T3 and T6 specifically.
- Scene graph types are born 3D-ready (transform slot exists) but only
  the axis-aligned path is implemented or tested in M1 — building 3D
  now is scope creep; building types that *forbid* 3D is a Thesis-1
  violation. The narrow line is T1's design job.
- Every task lands at least one golden or rig test in the same
  session (CLAUDE.md test discipline); goldens stay tolerance-0 while
  rendering is CPU.
- Deferred-by-name: `presentation-time` (M2, needs real vsync),
  dmabuf (M2), subsurfaces and popups (post-M1 unless a terminal
  demands popups — if foot won't run without one, stop and report
  rather than silently growing scope).
