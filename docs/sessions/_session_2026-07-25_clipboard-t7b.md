# Session summary — 2026-07-25 — Clipboard v1, subcompositor honesty, CI fix (M1 T7b)

**Task:** M1 T7b (prompt 11) — implement `wl_data_device_manager` properly
(Roland's option (a)), stop advertising what we do not honour, complete the
acceptance test, and close M1.

**Build/test result:** `make test` — **108 tests green** (up from T7's 101, +7),
clippy clean, zero warnings. The two subprocess suites (acceptance, clipboard) were
run 4× consecutively with no flakes.

**M1 remains COMPLETE.** It was closed in T7; this session hardened it — and fixed
the CI failure that closure produced.

---

## 0. The CI failure from the T7 push, and what it was

The T7 push went red. The acceptance test failed **in CI** at its own tripwire:

```
foot stopped committing after its first frame — frame callbacks are not flowing
(bytes_copied stuck at 1492704)
```

**It was the test, not the compositor.** The tripwire waited for foot to commit a
*second* frame unprompted. Locally the shell prompt arrived after frame one and
supplied that commit; in CI it did not — and an idle terminal with its prompt
already drawn has no reason to commit anything at all.

Fixed by making the test **cause** the redraw it waits for: settle → capture →
type → wait for the response. That is a stronger claim (typing is what M1 promises)
and it removes the dependency on incidental output. The frame-callback sabotage
check still trips it: with `FramePresenter::present`'s notice disabled, the test
fails with *"the terminal never redrew after 3 round(s) of typing… foot throttles
on [frame callbacks]"*. Reverted, re-ran green.

**A second flake, with a real defect underneath.** One run in eight failed
"pixels outside the damage region are unchanged". The cause was my assertion: it
compared the typing frame against the *settled* frame, so a repaint spread across
two ticks put the first tick's pixels outside the second tick's damage. The
comparison must be frame-versus-**previous**-frame. Now 8/8 and 4/4 clean.

Measured damage for a keystroke now ranges **0.11%–1.87%** of the output across
runs (bound: 25%) — the variance is foot repainting a cell, a line, or a prompt
row, and it is why the bound is generous.

### The second CI failure: the dev-binary test could not find the binary

The push after the fix above went red again, differently:

```
spawn parhelion-dev — was it built? : Os { code: 2, kind: NotFound }
```

`cargo test` builds a binary's *unit-test* harness, but it does not place
`target/debug/parhelion-dev`; only `cargo build` does. Locally that file existed
because I had run `cargo build` while developing — on a fresh runner it does not.
The test was reconstructing the path from its own executable's location, which
encoded that assumption invisibly.

Fixed properly rather than papered over: the test **moved to
`crates/backend-winit/tests/`**, the package that owns the binary, where
`env!("CARGO_BIN_EXE_parhelion-dev")` gives the path *and* cargo guarantees the
binary is built first. Verified by deleting `target/debug/parhelion-dev` and
re-running the full gate.

**What CI proved in the meantime:** the acceptance test and all six clipboard
tests — including the `wl-copy`/`wl-paste` round trip — **passed on the runner**.
The compositor work was sound; both CI failures were test-infrastructure
assumptions that only held on my machine.

## 1. Clipboard v1 (`wl_data_device_manager`)

Implemented through Smithay's data-device machinery. **The bytes never touch the
compositor**: a copy publishes a source, a paste asks the offer for a pipe, and the
clients transfer directly.

**Focus-gating is the v1 capability model** — only the keyboard-focused client may
set the selection, and only the focused client receives offers. This satisfies
I-7's letter (a grant, "has focus", checked in the core at request time); the
deeper design (security contexts, clipboard managers, primary selection, a smaller
grant set for Rayland clients) is deferred to M4's C8 work, as decided.

**A real bug found by the tests, and fixed.** Smithay clears a dead selection
*lazily* — only when the selection is next sent, which normally means on a focus
change. So when the clipboard's owner dies while focus does **not** change, the
focused client keeps an offer backed by a corpse. `State::refresh_selection` closes
it, and the timing is load-bearing: it must run **after** the departing client's
teardown, because the surface's `destroyed` hook fires while that client's data
source is still alive (checking there re-broadcasts the dying offer). Hence a
deferred flag drained at the end of the dispatch pass.

**Drag-and-drop is refused, not half-built:** `start_drag` cancels the source
immediately — protocol-legal, and the client learns at once instead of waiting on a
drag that will never resolve. A real drag is a pointer grab, and how grabs compose
with the focus model is its own design conversation. **Note for that conversation:**
Smithay does supply the grab machinery (`DnDGrab`), so the protocol half would be
cheap — what is not cheap is deciding what a drag *means* against C10 focus today
and S1's policy in M4, and against shaped/3D windows later.

### Tests (`crates/harness/tests/clipboard.rs`, 6)

| Test | What it pins |
|---|---|
| `a_copies_and_the_next_focused_client_pastes_the_exact_bytes` | The whole path, bytes checked through the pipe |
| `an_unfocused_client_can_neither_set_nor_receive_the_selection` | **The focus gate, asserted with client C** — not inferred |
| `replacing_the_selection_cancels_the_previous_source` | `cancelled` fires exactly once |
| `the_owners_death_clears_the_selection_without_disturbing_others` | The liveness bug above; B is told, and stays healthy |
| `starting_a_drag_cancels_the_source_rather_than_hanging` | The DnD deferral is honest, not silent |
| `wl_clipboard_tools_round_trip_real_bytes_through_the_compositor` | **`wl-copy` → `wl-paste`**: 39 bytes of UTF-8 between two third-party programs |

The last one is automated because `wl-clipboard` is installed here (the prompt
asked me to say so if it was). It skips loudly where the tools are absent.

## 2. `wl_subcompositor` — the instruction I could not carry out, with evidence

The task said to stop advertising it. **I implemented that, measured it, and
reverted it**, because it fails the milestone:

1. **It is separable** — `CompositorState::subcompositor_global()` +
   `DisplayHandle::remove_global` withdraws it cleanly. (Not welded; the prompt's
   anticipated blocker was not the one that occurred.)
2. **Withdrawing it breaks real clients**: `foot` refuses to start —
   `err: wayland.c:1746: no sub compositor`, exit 230 — so M1's acceptance
   criterion fails outright.
3. ~~**The gap is dormant, though**: `WAYLAND_DEBUG=1` shows foot calls
   `get_subsurface` **zero** times in a full session.~~
   **> CORRECTION (M2 T0): this was wrong.** The grep matched `@` where the debug
   format uses `#`, so it found nothing and I read that as evidence of absence.
   foot creates **nine** subsurfaces and puts buffers in **eight** — its client-side
   decorations, which Parhelion silently drops. The gap is not dormant; it is
   visible as an undecorated terminal. See the decision log's correction entry.

So the advertisement stays as a **stated debt**, documented in the code, the scene
doc (§12.3), and the decision log — and `conformance.rs` now pins the exact set of
advertised globals, so the next change to it is deliberate rather than a side
effect. **The real resolution is implementing subsurfaces, and that is your call**
(a natural pairing with popups); it is in the decision log's Pending section.

I am flagging this as a partial non-compliance with the prompt, not a completed
item: acceptance bullet "Registry no longer advertises `wl_subcompositor`" is
**not** met, deliberately, with the above as the reason.

## 3. Interactive checklist (extended)

```bash
# Terminal 1 — the compositor
cargo run -p parhelion-backend-winit --bin parhelion-dev
#   prints: parhelion-dev: WAYLAND_DISPLAY=wayland-N

# Terminal 2 — a real terminal inside it
WAYLAND_DISPLAY=wayland-N foot

# Terminal 3 — the clipboard, between two independent programs
WAYLAND_DISPLAY=wayland-N sh -c 'echo "hello from parhelion" | wl-copy'
WAYLAND_DISPLAY=wayland-N wl-paste          # → hello from parhelion
```

Then, inside foot: select text with the mouse and middle-click / `Ctrl+Shift+V` to
paste; copy from foot and `wl-paste` it in terminal 3.

**Only your eyes can settle these two** (unchanged from T7):
1. **Resize** the compositor window — the placeholder panel stays at (40, 40), the
   background grows, no tearing or stale strips.
2. **Cursor over the window** — the host desktop's cursor is what shows (we accept
   `set_cursor` and ignore it; the cursor plane is M2); nothing should flicker.

Everything else in the checklist — socket, clipboard round-trip, typing echo,
clean shutdown with no socket litter — is covered by automated tests.

## Files changed

### Created
- `crates/harness/tests/clipboard.rs` — the six tests above.
- `docs/sessions/_session_2026-07-25_clipboard-t7b.md` — this summary.

### Modified — core
- `crates/core/src/protocol.rs` — `SelectionHandler::new_selection` counting
  accepted selections; `refresh_selection` + the deferred `selection_needs_refresh`
  flag (the liveness fix); `ClientDndGrabHandler::started` cancelling drags; the
  `wl_subcompositor` finding documented at the site; **`Counters`** — the four
  observability atomics grouped into one type (clippy's "too many arguments" was
  right, and they are one thing).
- `ProtocolHost::selections_set()` — the definite condition the clipboard
  round-trip test waits on (waiting for `wl-copy`'s window was a sampling race: it
  maps, takes focus, copies, and destroys the window in about a millisecond).

### Modified — harness
- `crates/harness/src/protocol_rig.rs` — data-device support (manager, device,
  source, offer, `event_created_child!` for `data_offer`), `set_clipboard`,
  `read_clipboard` through a real pipe, `start_drag`, `advertised_globals`.
- `crates/harness/tests/acceptance.rs` — the CI fix (typing as the stimulus) and
  the frame-versus-previous-frame comparison.
- `crates/harness/tests/conformance.rs` — the advertised-global set pinned.

### Modified — docs
- `docs/scene_graph_v1.md` — §12.2 rewritten (clipboard, focus gate, liveness fix,
  DnD deferral), new §12.3 (the subcompositor debt), §12.4/§12.5 renumbered.
- `docs/parhelion_decision_log.md` — three entries (clipboard v1 focus-gated; DnD
  refused; the advertise-only-what-we-honour limit) + a Pending item for
  subsurfaces.
- `docs/diary.md` — "the compositor learns to refuse honestly, twice".

### Project map
- `data-device` node → parts for clipboard, the DnD **seam**, and the I-7 gating;
  `wl-clients` "Real terminal client" already `done`; `updated` bumped;
  `node --check` clean.
