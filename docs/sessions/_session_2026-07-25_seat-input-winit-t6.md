# Session summary — 2026-07-25 — Seat, input, and the nested winit backend (M1 T6)

**Task:** M1 T6 (prompt 09) — `wl_seat` with keyboard and pointer, the input
funnel and its dispatch-thread application, the C10 focus fallback, and
`crates/backend-winit`: a desktop window presenting the composited scene plus a
listening Wayland socket. Step 0 codified the project-map standing order in
CLAUDE.md.

**Build/test result:** `make test` — **89 tests green** (up from T5's 69, +20),
clippy clean, zero warnings. All prior tests pass unmodified; **no goldens
re-blessed** (the headless output keeps its fixed size, so the resize path cannot
touch them).

**Seam checks:** `smithay::desktop` and `smithay::backend::renderer` appear
nowhere in the workspace (grep-verified). Protocol objects are still touched only
on the dispatch thread — the winit loop and the rig both only *send* `InputEvent`
messages. Flush ownership unchanged: one `flush_clients` site.

## What was verified, and how

- **Headless (CI-safe), all of it automated:** 5 `FocusMap` unit tests, 7 input
  rig tests, 2 listening-socket tests (a real client over a real Unix socket in a
  temp dir — no `$XDG_RUNTIME_DIR` dependency), 3 keycode-table tests, 3
  softbuffer-conversion tests.
- **Interactive smoke, by me, bounded:** launched `parhelion-dev` on this machine
  for ~4 s. It bound and printed `WAYLAND_DISPLAY=wayland-3`, the socket file
  appeared, an external process connected to it successfully, and the binary
  stayed up until I stopped it (exact PID; the two socket files it left behind
  were removed — see the wart below).
- **What I could NOT verify:** what the window *looks like* — that it shows the
  placeholder panel, that the cursor moves over it, that resize behaves. Those
  need eyes on the screen and are Roland's to check:
  `cargo run -p parhelion-backend-winit --bin parhelion-dev`.
- **Known wart (not fixed, not M1's business):** killing the binary with SIGTERM
  leaves `wayland-N` and `wayland-N.lock` behind, because no signal handler means
  `Drop` never runs. A clean window-close unlinks them, and wayland-server's lock
  protocol makes a stale socket harmless on the next bind.

## Files changed

### Created
- `crates/core/src/input.rs` — the `InputEvent` funnel, `BTN_*` constants,
  `FocusMap` (the §7 read-mostly routing table) and `Hit`; 5 unit tests.
- `crates/backend-winit/` — new workspace member:
  - `Cargo.toml` (winit 0.30, softbuffer 0.4; explicitly *not*
    `smithay::backend::winit`, with the reason);
  - `src/keycode.rs` — winit `KeyCode` → evdev table, `KeyTranslator` with its
    dropped-key counter; 3 unit tests;
  - `src/present.rs` — `Frame` (RGBA8) → softbuffer `0x00RRGGBB`; 3 unit tests;
  - `src/lib.rs` — `NestedBackend`: window, softbuffer surface, input
    translation, resize, render tick; carries the §7 deviation note;
  - `src/bin/parhelion-dev.rs` — the thin dev binary.
- `crates/harness/tests/input.rs` — 7 tests: seat capabilities + real xkb keymap;
  keys with evdev codes, monotonic serials, modifiers before the key they modify;
  focus following topmost across map/unmap (the enter/leave *sequence*); pointer
  crossing two cascaded windows with surface-local coordinates; click reaching the
  window under the cursor only after its enter; axis; roleless/unmapped surfaces
  receiving nothing.
- `crates/harness/tests/socket.rs` — 2 tests: a client over a real listening
  socket maps a window; the socket keeps accepting after the first client.
- `docs/sessions/_session_2026-07-25_seat-input-winit-t6.md` — this summary.

### Modified — core
- `crates/core/src/lib.rs` — the `input` module.
- `crates/core/src/protocol.rs` — real seat (`new_wl_seat`, keyboard with an
  explicit `us` layout, pointer) + `delegate_seat!`; `SeatHandler` filled in
  (`focus_changed`/`cursor_image` deliberately empty, with reasons); `apply_input`
  applying the funnel to the seat handles; `refocus_keyboard` (C10);
  `FocusMap`/`sid_to_obj` bookkeeping wired to map, unmap, destroy;
  `Control::{Input, Listen}`; `ProtocolHost::{input, listen_auto, listen_at}`;
  `admit_client` factored out so socketpair and socket clients share one path;
  `SEAT_NAME`, `KEY_REPEAT_DELAY_MS`, `KEY_REPEAT_RATE_HZ`.
- `crates/core/src/render.rs` — `RenderLoop::compositor_mut` (for resize).

### Modified — backends and harness
- `crates/backend-headless/src/composite.rs` — `CpuCompositor::resize`, and a
  `force_full` flag so a fresh or resized frame repaints in full regardless of the
  damage it is handed (the guarantee lives with the retained frame, not with the
  caller).
- `crates/harness/src/protocol_rig.rs` — `wl_seat`/`wl_keyboard`/`wl_pointer`
  bound at connect; `SeatEvent` (one ordered log, because ordering is what the
  tests check); keymap capture; `input_events`, `clear_input_events`, `keymap`,
  `seat_capabilities`, `surface_id`, `pump_until_input_events`.

### Modified — docs, CI, workspace
- `CLAUDE.md` — **Step 0**: the project-map standing order, merged with the
  section that already existed (see the note below); `project-map.js` /
  `project-map.html` added to the supporting-documents list.
- `.github/workflows/ci.yml` — the first apt step (`libxkbcommon-dev`); the "no
  apt step, on purpose" comment **amended, not deleted**, and corrected: the
  dependency was already implicit.
- `Cargo.toml` — `crates/backend-winit` as a workspace member.
- `docs/scene_graph_v1.md` — new **§11** (funnel, §7 deviation, focus + replica,
  the nested backend, the coordinate trap, T6 tests); §12 rewritten to name each
  temporary part's successor; header lines updated.
- `docs/parhelion_decision_log.md` — four entries: the funnel + winit-loop
  deviation; the routing replica (Roland's ruling); the C10 focus policy; the CI
  system dependency.
- `docs/diary.md` — T6 entry (`#input`, `#design-decision`, `#tradeoff`,
  `#invariant`, `#bug`, `#discovery`, `#ci`, `#backend`).
- `docs/parhelion_project_index.md` — current state, documents, subsystem rows.

### Project map (per the standing order codified this session)
- `seat-input` → `done`, with parts (seat + keymap, the funnel, focus fallback,
  pointer hit-testing) and files/specs.
- `winit` → `done`, with parts (window + softbuffer presentation, input
  translation, resize, dev binary + socket).
- New `t-input` node at `seam`: the interface exists, the thread arrives in M2.
- `harness` gained input-injection and socket-test parts; `render` gained the
  resize part.
- `project.updated` bumped to 2026-07-25; `node --check project-map.js` clean.

## Notes for Roland

1. **CLAUDE.md already had a "Project map" section** (added last session,
   `071a649`) in the exact place the prompt said to insert one. I merged rather
   than duplicated: the prompt's text is the spine, plus the two things only the
   existing version said (the `active`/`planned` status definitions, and that
   editing the data file is enough because the HTML derives from it). Reporting
   because the prompt asked me to flag exactly this.
2. **`delegate_xdg_shell!` had already forced an empty `SeatState` in T5.** This
   task filled it in, which is why the seat arrived with no structural churn.
3. **T7 is the milestone acceptance run** — `foot` or `weston-terminal` under this
   backend. The binary prints the `WAYLAND_DISPLAY` to point it at.
