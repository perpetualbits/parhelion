# Prompt 15 — libinput, the real T-input, and the cursor plane (M2 T2)

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-25.
**Gate:** run after Roland's T1 smoke verdict is recorded (the four
connector facts + checklist). If the smoke surfaced problems, those
are a report-and-fix first; this prompt waits.
**Reads first:** `docs/plans/m2_tasks.md` T2; CORE-BOUNDARY §7
(T-input) and I-2; `scene_graph_v1.md` §11.2 (the deviation this task
retires) and §13 (T-commit's world — the `!Send` session constraint
below flows from it).

---

## Context

Input arrives on metal, and with it the invariant this architecture
was partly built for: **I-2, cursor motion never waits on
rendering.** A dedicated T-input thread owns libinput and produces
the existing `InputEvent` funnel — the M1 interface finally meets its
intended producer, the winit path remaining for nested dev. Cursor
motion takes a hardware cursor plane, bypassing the render path
entirely. The §7 deviation and the map's `t-input` seam both retire.

## Design constraints

1. **The `!Send` session shapes the thread split.** libinput opens
   device fds through the seat session — which is `Rc`-based and
   lives on T-commit (T1's finding). Resolution: T-input runs its own
   libinput context with a **custom `LibinputInterface`** whose
   `open_restricted`/`close_restricted` proxy over a small
   request/reply channel to the session's owner. Open/close happen
   only at device add/remove, so a blocking round-trip there is
   harmless — input from an already-open device never crosses that
   channel. If Smithay's libinput/session plumbing genuinely fights
   this split (its canonical `LibinputSessionInterface` assumes
   cohabitation), stop and report — thread ownership is CORE-BOUNDARY
   law, not an integration convenience.
2. **Cursor motion's path is: T-input → position message → T-commit →
   cursor-plane-only atomic update.** No render involvement at any
   point; T-commit's cursor updates are independent of frame pacing
   (cursor-only commits are the driver-special-cased fast path).
   Satisfying I-2 does not require bypassing T-commit — it requires
   bypassing *rendering*; T-commit never waits on T-render, so the
   two-hop path is compliant. Instrument it: a
   funnel-to-cursor-commit latency counter/histogram.
3. **Cursor content:** a small dumb buffer (BO) for the cursor plane
   (respect the device's reported cursor size). Default core-owned
   cursor image (drawn, simple, ours). Client `set_cursor` honoured
   per protocol: serial validated against a recent pointer enter,
   surface takes the cursor role, its committed pixels are copied
   (dispatch-thread copy, as ever) and travel to T-commit for BO
   upload; hotspot respected; `set_cursor(null)` hides (plane
   disabled). Cursor visibility/content is per pointer-focus rules.
4. **Software-cursor fallback, behind the hardware path:** if no
   cursor plane exists (or claiming fails), the cursor becomes a
   core-owned topmost scene node updated on motion — correct but
   damage-costly, and *visible in counters* (a `cursor_mode` counter
   states which path is live; the fallback existing must never mask
   a plane regression on hardware that has one).
5. **libinput lifecycle:** udev-backed device discovery and hotplug
   (add/remove mid-session), suspend on VT pause / resume on VT
   resume (riding T1's session events). Keyboards, pointers, and
   scroll (wheel + finger) map to the existing funnel — libinput
   speaks evdev natively, so the M1 translation table is bypassed,
   which sets up:
6. **Producer parity, tested:** one shared test drives the same
   logical key/button/axis sequence through the winit-producer
   translation and a synthesized libinput-producer path, asserting
   identical `InputEvent`s. The funnel's meaning must not depend on
   who feeds it.

## Task

1. T-input thread (`parhelion-input`, named like its siblings):
   libinput context, custom interface with the open/close proxy,
   udev hotplug, VT suspend/resume; funnel production.
2. Cursor plane path per constraints 2–4, with the latency
   instrumentation and mode counter.
3. `set_cursor` protocol handling (role, serial, hotspot, hide).
4. **The I-2 test, CI-runnable:** a `--slow-render` style injection
   (or test-harness equivalent) stalls T-render; a headless cursor
   sink records update timestamps; the test asserts cursor updates
   continue at full cadence while frames stall. On metal this is the
   showpiece — checklist below.
5. Producer-parity test (constraint 6); funnel rig suite untouched
   and green (the interface didn't change, prove it).
6. Docs: `scene_graph_v1.md` — §11.2 gains its "resolved in M2 T2"
   coda (what the pure model said, now true); cursor section; input
   thread in the ownership diagram; decision-log entries (the
   open-proxy design; the fallback policy); diary; session summary;
   map — `t-input` seam → done, cursor nodes, `updated`,
   `node --check`.
7. **Interactive checklist** (the fun one): on the TTY — cursor
   appears and glides (hardware plane: check the counter says so);
   typing in foot works *on metal* for the first time; rt from foot,
   focus handover by pointer; scroll in foot; **the I-2 demo** — run
   with the render stall injected: windows freeze, cursor keeps
   gliding (this is the architecture visible to the naked eye —
   worth ten seconds of appreciation); VT away/back with input
   recovering; unplug/replug a USB keyboard if one is handy.

## Acceptance

- `make test` green (142 + new); clippy clean; CI green (I-2 test
  and parity test run headless).
- Thread ownership grep-visible: libinput context on T-input only;
  session calls only via the proxy; no Wayland objects off dispatch.
- Latency histogram exists; cursor-mode counter exists; the I-2 test
  fails if cursor updates are routed through the render tick
  (sabotage-verified, stated, reverted).
- Parity test green; funnel rig untouched.
- §11.2 coda written; seam node filled.
- Checklist delivered; smoke verdict recorded afterward.

## Out of scope

Frame scheduler and presentation-time (T3); touch, tablets,
gestures; pointer constraints/relative-pointer (games — later
milestone); cursor themes (client-set and default only); multi-seat;
software cursor *optimization* (the fallback is correct, not tuned —
M5 territory if it ever matters).
