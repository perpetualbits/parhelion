# Prompt 09 — Seat, input, and the winit nested backend

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M1, task T6 (see `docs/plans/m1_tasks.md`).
**Reads first:** `docs/plans/m1_tasks.md` T6 (including its
honesty clause); `docs/scene_graph_v1.md` §10.3 (the SeatState
already in the tree); CORE-BOUNDARY §7 (T-input) and C10.

---

## Step 0 — Codify the project-map standing order in CLAUDE.md

The map (`project-map.html` + `project-map.js`, repo root) has been
maintained by session practice; make it law. Insert the following
subsection into CLAUDE.md's Workflow rules, after "Project index"
(verbatim; adjust only if it contradicts the file's current local
state, and report if so). Add `project-map.js` / `project-map.html` to
the supporting-documents list with a one-liner. This is a small,
clearly-scoped change.

```markdown
### Project map

`project-map.html` + `project-map.js` at the repo root render an
interactive map of the architecture. The `.js` file is pure data
(`window.PROJECT_MAP`) and the single source of truth the renderer
reads.

Standing order — every session that changes code, docs, or the status
of any planned work:

- Update the affected nodes and parts before closing the session:
  status transitions (planned → active → done; seams filled), new
  nodes when a new subsystem, crate, or canonical doc appears, and
  `files` / `specs` paths when they change.
- Status is derived, never invented: `done` only for work in the tree
  and green under `make test` in this session; `seam` for interfaces
  deliberately reserved for later filling.
- Bump `project.updated` to the session date.
- The file stays pure data and must remain syntactically valid —
  verify with `node --check project-map.js` (or equivalent) before
  closing.
- The session summary lists map changes alongside file changes.
```

## Context

Two things arrive together because winit delivers both: input (seat,
keyboard, pointer) and the nested backend (a window Roland can see).
By the end, `map_toplevel` clients receive typed keys and pointer
events in the rig, and a dev binary presents the composited scene in
a desktop window with a listening Wayland socket — T7 will point foot
at it.

## Design constraints

1. **All protocol interaction stays on the dispatch thread** (T2's
   rule, unchanged). Input events — from winit or from the rig — are
   messages *into* the dispatch thread, which drives the Smithay seat
   handles (`KeyboardHandle::input`, pointer motion/button/axis).
   One `InputEvent` enum is the funnel; winit translation and rig
   injection both produce it. This is T-input's *interface* arriving
   before T-input's *thread*.
2. **The §7 honesty clause, invoked:** in the nested backend, winit's
   event loop must run on the main thread and owns both input intake
   and window presentation — M1's "T-input" is therefore winit's loop
   feeding the funnel, not a dedicated thread. Document the deviation
   in `scene_graph_v1.md` exactly as the task plan requires: what the
   pure model says, what winit forces, and that the real T-input
   arrives with libinput/DRM in M2. Not silent, not apologetic — a
   recorded, bounded deviation.
3. **Raw winit + softbuffer, not `smithay::backend::winit`.** Smithay's
   winit backend is welded to its GLES renderer — the bypassed layer.
   Ours: winit window, softbuffer surface, blit the retained `Frame`
   (RGBA → softbuffer's u32 layout — one conversion function, one
   comment about channel order, one test). Window resize → output
   resize → structural full damage; the headless output keeps its
   fixed size so goldens are untouched.
4. **Keyboard reality:** wl_keyboard requires an xkb keymap.
   Use xkbcommon with a default `us` keymap via Smithay's keyboard
   handle machinery; enable only the Smithay/xkb features this needs
   and record them. **This is CI's first system dependency** —
   `libxkbcommon` — so `ci.yml` gains its first apt step, and the
   proud "why there is no apt step" comment is *amended, not deleted*:
   it now names the one dependency and why it earned entry.
   Winit `KeyCode` → evdev keycode translation is ours: a match table
   covering the standard typing set (letters, digits, punctuation,
   modifiers, space/enter/backspace/tab/escape, arrows); unmapped keys
   are dropped and counted (a counter, not a panic — exotic keys are
   M2+ business).
5. **Focus is C10 fallback, loudly temporary:** keyboard focus =
   topmost mapped toplevel, updated on map/unmap; pointer focus =
   topmost node under the cursor, enter/leave on crossings, with
   correct serial discipline. Constants/logic module-doc'd as "S1
   (M4) replaces this." Client `set_cursor` requests are accepted and
   ignored for rendering (nested mode shows the OS cursor — doc
   note; the cursor plane is M2).
6. **Pointer axis included** (wheel/scroll from winit → axis events)
   — terminals scroll; it is cheap here and painful retrofitted.
   Keyboard repeat: advertise `repeat_info`; actual repeat is
   client-side per current protocol — comment says so.

## Task

1. Real seat: `Seat::new`, wl_seat global with keyboard + pointer
   capabilities, keymap delivery, modifiers tracking (xkb state via
   the Smithay handle).
2. The `InputEvent` funnel + dispatch-thread application to seat
   handles; hit-testing against the scene for pointer focus (topmost
   mapped node containing the point — scene already knows stacking).
3. Focus policy per constraint 5, wired to map/unmap.
4. `crates/backend-winit`: window + softbuffer presentation of the
   retained frame, driven by the existing render tick; winit input →
   funnel; resize handling; a dev binary (name it plainly —
   `parhelion-dev` is fine) that opens the window **and** a listening
   Wayland socket (`WAYLAND_DISPLAY` printed on stdout) so external
   clients can connect. The binary is the M1 interactive artifact;
   keep it thin — all logic lives in the library crates.
5. Rig tests (headless, no winit in CI):
   - Seat capabilities + keymap event received (assert non-empty,
     parseable header).
   - Key press/release reach the focused client with correct evdev
     code, serial monotonicity, and modifiers events on shift.
   - Focus follows topmost: map A, map B (B focused), unmap B (A
     re-focused) — keyboard enter/leave sequence asserted.
   - Pointer: motion across two cascaded toplevels → leave/enter
     pair with surface-local coordinates asserted; button click
     delivered to the surface under the cursor; axis event delivered.
   - Enter-before-input ordering (no key/button before the
     corresponding enter).
   - Unmapped/roleless surfaces never receive input (the T5 rule
     meets input).
6. Interactive smoke (manual, by Roland): run the binary — window
   appears with core-owned test content; cursor moves over it; resize
   works. State in the summary what you could and could not verify
   yourself headlessly.
7. Docs: `scene_graph_v1.md` — input section (funnel, focus fallback,
   the §7 deviation note, set_cursor note); decision-log entries
   (CI dependency; the winit-loop deviation); diary (`#input`
   `#backend`, and the keycode table will likely earn a `#tradeoff`);
   session summary; `make test` stated. Project map per the standing
   order you just codified: seat/input and winit-backend nodes to
   their earned statuses, the T-input seam noted as an interface
   whose thread arrives in M2, `updated` bumped, `node --check`
   clean.

## Acceptance

- All prior tests green plus the rig suite above; clippy clean.
- CI green with the single new system dep, comment amended.
- Grep: still no `smithay::backend::renderer`, no `smithay::desktop`;
  protocol objects still touched only on the dispatch thread.
- The deviation note exists and says what replaces it (M2).
- Dev binary runs and serves a socket (headless-verifiable part:
  socket accepts a rig client while the winit window is stubbed or
  feature-gated — structure this so the binary's plumbing is testable
  without a display).
- CLAUDE.md carries the project-map standing order; the map reflects
  this session per that order and passes `node --check`.
- Unmapped keys counted, not fatal.

## Out of scope

libinput and the dedicated T-input thread (M2); cursor plane and
client cursor surfaces (M2); touch, tablets, gestures; key repeat
generation; focus policy beyond C10 (M4); popups (unchanged standing
note); any DRM work.
