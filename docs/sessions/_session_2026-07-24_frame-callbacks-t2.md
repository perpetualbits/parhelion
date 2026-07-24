# Session summary — 2026-07-24 — Reverse path: frame callbacks, flush ownership, backpressure (M1 T2)

**Task:** M1 T2 (prompt 05) — open the reverse direction proven-unbuilt in M0:
`wl_surface.frame` callbacks fired from the render side, flush ownership settled
to one site, and the backpressure policy (the I-10 fairness rider) written into
code and `scene_graph_v1.md`.

**Build/test result:** `make test` — **40 tests green** (up from T1's 37, +3 T2
rig tests), clippy clean. Acceptance greps: exactly one `flush_clients` call
site (`crates/core/src/protocol.rs`), zero `DisplayHandle` use outside
`protocol.rs`. The flooding test was verified to **fail** when the throttle is
removed (backlog `2065` vs bound `64`), then the change reverted.

**Design decision confirmed with Roland:** backpressure fidelity — "keep it
simple, note it": ship the straightforward per-client throttle, accept the
dispatch-thread CPU spin during an active flood (frame path unaffected), document
it with the M2 refinement noted.

## Files changed

### Created
- `docs/sessions/_session_2026-07-24_frame-callbacks-t2.md` — this summary.

### Modified
- `crates/core/src/protocol.rs` — the reverse path and backpressure:
  - `FramePresenter` (atomic timestamp + `calloop` ping; wait-free `present(t)`)
    and its Send/Sync guard; the render→dispatch notice.
  - `present()` — drains each live surface's committed `frame_callbacks` and
    sends `wl_callback.done(t)`; v1 fires all pending callbacks per tick
    (not visibility-gated), documented.
  - `pump_display` rewritten: per-client dispatch via `dispatch_single_client`,
    skipping any client whose pending-callback backlog ≥ `MAX_PENDING_FRAME_CALLBACKS`
    (the throttle); republishes the total for observability. No flush here.
  - Single flush site moved to the loop body (`state.dh.flush_clients()` after
    `event_loop.dispatch()`), with the "one flush" comment.
  - `State` gains `surfaces` (ObjectId → WlSurface, dispatch-thread-only),
    `present_ts`, `pending_frame_callbacks`; `new_surface`/`destroyed` maintain
    the surface map.
  - `ProtocolHost` gains `frame_presenter()` and `pending_frame_callbacks()`;
    `new` creates the ping + shared atomics and threads them into `run_dispatch`.
  - `pub const MAX_PENDING_FRAME_CALLBACKS = 64`, with reasoning; module docs
    grown (reverse edge, flush ownership, backpressure).
- `crates/core/src/render.rs` — `RenderLoop::tick(time_ms)` (was `tick()`);
  optional `FramePresenter` via `with_presenter`; `tick` calls `present(time_ms)`
  when attached (non-blocking, I-1). Unit test updated to `tick(0)`.
- `crates/harness/src/protocol_rig.rs` — `App` now records `wl_callback.done`
  timestamps (`frame_dones`); `ScriptedClient` gains `frame()`, `flush()`,
  `frame_dones()`; `_conn` → `conn` (used by `flush`).
- `crates/harness/tests/protocol.rs` — three T2 rig tests:
  `scene_triggered_frame_callback_reaches_client` (reverse-direction proof,
  attach-less), `frame_callback_lifecycle_conformance` (no `done` before the
  carrying commit; one-shot), `flooding_client_is_throttled_second_client_served_and_bounded`
  (bounded backlog, B served in one tick, A not killed; fails if the bound is
  removed).
- `crates/harness/tests/scene_render.rs` — `render.tick()` → `render.tick(0)`
  (render-only goldens; no presenter).
- `docs/scene_graph_v1.md` — new §8 "The reverse path — frame callbacks, flush
  ownership, backpressure (T2)" (5 subsections + T2 tests); old §8 → §9;
  §7 test list and re-entrancy header updated.
- `docs/parhelion_decision_log.md` — new 2026-07-24 T2 section: three
  load-bearing decisions (enqueue-only render side + single flush; callback v1
  semantics; per-client backpressure via socket-unschedule, with the
  read-to-`WouldBlock` discovery recorded).
- `docs/diary.md` — M1 T2 narrative (reverse edge thinness, visibility-gating
  reversal, flush relocation, the backpressure discovery, the throttle-signal
  choice, the v1 spin cost).
- `docs/parhelion_project_index.md` — current-state and prompts entries updated
  for T2.

## Invariants touched (cited in code)

- **I-1** — the present notice is wait-free from T-render (atomic + eventfd
  ping; no shared lock, no block).
- **§7** — every Wayland object stays on the dispatch thread; only a `u32` + a
  ping cross to it.
- **I-3** — the proto→scene emit stays fire-and-forget; backpressure is by
  socket-unschedule, never by blocking the dispatch thread.
- **I-10** — per-client pending-callback accounting is its fairness rider.

## Notes / follow-ups (M2)

- Real vsync pacing (`presentation-time`) and occlusion-aware callback
  throttling (needs T4 damage/visibility).
- Tighter throttle: per-client readiness / edge management to remove the
  active-flood dispatch-thread spin.
