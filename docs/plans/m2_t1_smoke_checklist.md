# M2 T1 — Interactive smoke checklist (DRM/KMS on the dev machine)

> **Re-entrancy header.**
> **Status:** v1.0 · **Date:** 2026-07-26 · **Kind:** operational checklist (not a design doc).
> **Upstream:** `docs/plans/m2_tasks.md` T1; `docs/scene_graph_v1.md` §13 (the canonical description of what is being exercised).
> **What this is:** the protocol for the part of M2 T1 that no test can reach. Every step states what to expect **and** how to get out. The verdict is recorded afterwards, in the session summary, the way T6's and T7's were.

---

## Read this first — two absences that are not bugs

**There is no keyboard input on metal.** libinput and the real T-input thread are
**M2 T2**. `foot` will launch, map, and render, and **typing into it will do
nothing**. The winit backend has input; the DRM backend does not, yet. If you sit
down at a TTY and type into a terminal that ignores you, that is this, not a
regression.

**There is no cursor.** The hardware cursor plane is also **T2**. Moving a mouse
moves nothing on screen.

This smoke is about three things and no others: **pixels** (does a mode-set happen
and is the picture correct), **pacing** (does it keep producing frames from real
vblanks), and **VT survival** (does switching away and back leave a working
screen and a working console).

---

## Before you start

- [ ] `make test` is green on this checkout (142 tests, clippy clean).
- [ ] Build the binary once, *before* leaving the desktop — compiling on a TTY
      with the compositor about to take the screen is avoidable stress:
      ```sh
      cargo build --bin parhelion-dev
      ```
      The binary lands at `target/debug/parhelion-dev`.
- [ ] Know your escape hatches. Written down here because the moment you need one
      is the moment you cannot look them up:
      - **VT switch**: `Ctrl-Alt-F2` (or any other free VT) suspends the
        compositor and gives you a normal console. This is also step 5's test.
      - **ssh**: from another machine, `ssh <this-box>` reaches a shell that can
        `kill <pid>`. Worth having a session already open.
      - **`--exit-after=SECONDS`**: the compositor shuts itself down on a timer,
        needing no keyboard at all. Step 2 uses it deliberately.
      - **SIGTERM**: `kill <pid>` from any of the above ends it through the normal
        path (socket unlinked, DRM master released, console restored).
- [ ] Note which card you expect. This machine has two (`/dev/dri/card1` is the
      Intel iGPU driving the panel; `card2` is the NVIDIA). The backend picks the
      first card with something plugged in; `--drm-device /dev/dri/cardN` overrides.

---

## Step 1 — Switch to a TTY

`Ctrl-Alt-F3` (any VT that is not your graphical session), log in, and `cd` to the
checkout.

**Expect:** an ordinary text console.

**Why a different VT and not the one your desktop is on:** the compositor needs a
session it can take DRM master on, and your desktop already holds it on its own VT.

---

## Step 2 — First run, on a timer

```sh
./target/debug/parhelion-dev --drm --exit-after=20
```

**Expect, on stdout before the screen changes:**

- `parhelion-dev: WAYLAND_DISPLAY=wayland-N` — **write N down**, step 4 needs it.
- `parhelion-dev: DRM backend — NO INPUT and NO CURSOR yet (both are M2 T2)`
- `parhelion-drm: seat seat0`
- one line per connector the card reports, e.g.
  `parhelion-drm: /dev/dri/card1 eDP-1 — connected connector: 42 mode(s)`
- `parhelion-drm: eDP-1 on /dev/dri/card1 — 1920x1200 @ 59.953 Hz (stride 7680 bytes)`

**Expect, on screen:** the console is replaced by a dark blue-grey field
(`#181a20`) with a lighter grey rectangle near the top-left (240×160 at offset
40,40 — the core-injected C10 placeholder). Nothing moves. **No cursor.**

**Expect, after twenty seconds:** `parhelion-dev: --exit-after elapsed; shutting
down`, a closing line counting frames presented, commits rejected, and VT
switches, and the text console back.

**Record for the diary:** the connector name, the mode line, the refresh to three
decimals, and the stride. Whether the stride is padded (≠ width × 4) is exactly
the hardware-honesty note this milestone asked for.

**If the screen stays black:** the compositor is probably still running (the timer
will end it). Read the scrollback afterwards — every failure names what it was
attempting: `cannot open a libseat session: …`, `cannot find a usable DRM device:
…`, `cannot use atomic mode-setting: …`.

**If it exits immediately** with `cannot find a usable DRM device`, try
`--drm-device /dev/dri/card1` and then `card2`.

---

## Step 3 — Second run, no timer

```sh
./target/debug/parhelion-dev --drm
```

**Expect:** the same picture, held indefinitely. Note the PID (`echo $!` if you
backgrounded it, or find it from another VT / ssh).

Leave it running for the next two steps.

---

## Step 4 — A real client

From **another VT or an ssh session** (there is no keyboard on the compositor's
screen, so it cannot be launched from there):

```sh
WAYLAND_DISPLAY=wayland-N foot
```

**Expect on the compositor's screen:** a `foot` window appears at the top-left,
**with its title bar and borders** (subsurfaces landed in M2 T7). It renders its
prompt and a blinking cursor.

**Expect: typing does nothing.** Keys pressed on the compositor's VT reach
nothing; keys typed into the launching terminal go to that terminal's own shell.
This is the T2 gap, stated at the top of this file.

**A useful thing to watch for:** the block cursor should blink. That is a client
redrawing on frame callbacks it can only be getting because vblanks are driving
the render tick. If the window renders once and then freezes, the frame cycle has
stalled — which is the failure mode the watchdog (§13.4) exists to prevent, and
worth reporting with the closing counter line.

**Escape hatch:** `kill` the `foot` process from the same shell.

---

## Step 5 — VT round-trip

With the compositor still running and `foot` mapped:

1. `Ctrl-Alt-F1` (or any VT other than the compositor's).
   **Expect:** a normal text console, immediately. On the compositor's stdout (if
   you can see it): `parhelion-drm: session paused (VT switched away)`.
2. Wait a few seconds — long enough to be sure it is not a race.
3. `Ctrl-Alt-F3` back to the compositor's VT.
   **Expect:** `parhelion-drm: session resumed (VT switched back)`, and the
   **full picture returns in one redraw** — background, placeholder, and the
   `foot` window with its decorations, all correct. Not a partial screen, not
   stale garbage from the other VT, not a black screen that fills in gradually.

**What "one redraw" is testing:** on resume the backend tells the scene everything
is damaged and forces a full modeset. If the screen comes back partially correct,
the damage-on-resume is wrong; if it comes back black and stays black, the modeset
is.

Do this **twice** — the second round-trip is the one that catches state that was
only correct because it was still the first.

---

## Step 6 — Clean shutdown

From another VT or ssh: `kill <pid>` (SIGTERM).

**Expect:**

- a closing counter line: `parhelion-drm: N frame(s) presented, 0 commit(s)
  rejected, 4 VT switch(es)`;
- the text console restored on the compositor's VT — the kernel takes the screen
  back when DRM master is released;
- **no socket litter**: `ls $XDG_RUNTIME_DIR/wayland-*` shows no `wayland-N` or
  `wayland-N.lock` belonging to this run.

`Ctrl-C` in the compositor's own terminal does the same thing — but only if you
are on that VT, which you cannot type on. SIGTERM from elsewhere is the reliable
one.

---

## What to report

For each step: pass, fail, or "something odd". For step 2 also the four recorded
facts (connector, mode, refresh, stride) — they go in the diary. If anything
failed, the closing counter line and the last few stdout lines are the useful
part; `commits rejected > 0` in particular means the kernel refused an atomic
commit and the watchdog was papering over it, which is a real finding.

## What this checklist does **not** cover

Input of any kind (T2), the cursor (T2), frame-scheduler slack or
`presentation-time` (T3), anything GPU (T4–T6), a second monitor or hotplug (M9),
and suspend/resume (M9). None of those exist yet; their absence is not a result.
