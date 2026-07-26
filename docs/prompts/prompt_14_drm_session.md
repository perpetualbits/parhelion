# Prompt 14 — Session, DRM/KMS atomic, dumb buffers (M2 T1)

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-25.
**Milestone:** M2 T1 — the metal begins.
**Reads first:** `docs/plans/m2_tasks.md` T1 + standing notes;
CORE-BOUNDARY §7 (T-commit is born here) and C1; decision log
"Smithay threading fit" entry 2 (hardware backends were
consume-listed "later" — this is later); the T7 note about 60 Hz.

---

## Context

`parhelion-dev` stops being the artifact: this task boots Parhelion
from a TTY on the dev machine, presenting the existing CPU-rendered
frames through DRM/KMS atomic commits into dumb buffers, under a
libseat session, surviving VT switches. No GPU, no planes beyond
primary, no scheduler cleverness (T3) — the smallest honest path from
`Frame` to glass. The advertised refresh rate becomes a measured
fact: `wl_output`'s mode comes from the connector's actual mode, and
the render tick is driven by real vblank events instead of the test
clock.

## Design constraints

1. **Consume Smithay's hardware layers, still not its renderer.**
   `session::libseat`, `backend::drm` (device, atomic surface,
   vblank events), and the dumb-buffer allocator are exactly the
   "hardware backends later" the decision entry reserved. The seam
   rule is unchanged and now harder to keep by accident: dumb-buffer
   framebuffer creation must not pull `backend::renderer` — if
   Smithay's plumbing makes that awkward, stop and report before
   wrapping.
2. **T-commit is born, owning the DRM world.** One thread owns the
   DRM fd, atomic submission, vblank event handling, and the session
   pause/resume source — its own calloop loop, per §7. The render
   thread hands completed `Frame`s over a channel (double-buffered
   dumb buffers: copy the frame into the back buffer's mapping,
   commit, flip on vblank). Vblank drives the render tick on metal;
   the headless/test tick is untouched. Ownership grep-visible, as
   T-render's was.
3. **Connector/mode v1:** first connected connector, preferred mode;
   `wl_output` advertises that mode's true geometry and refresh —
   the 60 Hz claim retired. Multi-output, hotplug, mode setting
   beyond preferred: M9. Log (diary-grade) what the dev machine's
   connector actually reports; the hardware-honesty rule starts
   here.
4. **VT switching per session events:** pause → stop commits, drop
   what logind revokes; resume → reacquire, full structural damage,
   one clean frame, resume pacing. A pending frame during pause is
   dropped, not queued — bounded, simple, documented.
5. **CI stays headless and green.** `backend-drm` compiles in CI
   (build check), never runs there. Extract and unit-test the
   testable logic (mode selection policy, damage→dirty-rect
   conversion for `FB_DAMAGE_CLIPS` if you add it now — optional,
   M5 owns the payoff — buffer stride/format math). State plainly
   in the summary what is CI-verified vs. dev-machine vs. eyes.
6. **First-boot safety.** A compositor holding your TTY hostage is a
   rite of passage we can skip: `parhelion-dev` grows a `--drm` flag
   (one binary, backend selected; winit remains default) and an
   `--exit-after=SECONDS` flag for early boots. The checklist's
   first run uses it; the escape hatches (VT switch itself, ssh) are
   written down before they're needed.

## Task

1. `crates/backend-drm`: session, device, atomic surface, dumb
   buffers, vblank loop, VT pause/resume — behind the same backend
   interface winit/headless present (if the T1 interface needs to
   grow for vblank-driven ticking, grow it deliberately and
   document; the headless test tick must keep working unchanged).
2. `wl_output` fed from the real mode; refresh advertised truthfully.
3. `--drm` and `--exit-after` flags; graceful shutdown on metal
   (signals + session release; socket cleanup as before).
4. Tests: unit tests for extracted logic; the full headless suite
   untouched and green; a build-only CI check for the DRM crate.
5. Docs: backend section in the scene/architecture doc (thread
   ownership diagram gains T-commit; frame handoff; VT semantics);
   **codify the re-stated-state rule** from T7's 76% finding as a
   named rule in the damage section ("state re-stated by the
   protocol must be a no-op when unchanged — equality-check before
   damage"); decision log if anything rises (the backend-interface
   growth likely does); diary — this session is what the
   hardware-honesty rule was written for; session summary; map
   (drm/session nodes, t-commit seam → done, `updated`,
   `node --check`).
6. **The interactive protocol, carefully written** (this task's real
   acceptance is your eyes at a TTY): switch to a TTY, first run
   with `--drm --exit-after=20`, expect core content + cursor
   *absent* (cursor plane is T2 — expected, listed); second run
   without the timer, `WAYLAND_DISPLAY=… foot` launched from a
   second VT or ssh — foot maps and renders decorated, but **no
   typing: input on metal does not exist until T2** (libinput). The
   silent keyboard is expected and the checklist says so in bold;
   this smoke is about pixels, pacing, and VT survival. VT switch away and back (content
   returns, one full redraw); clean kill, no socket litter, TTY
   restored. Every step with its expected outcome and its escape
   hatch.

## Acceptance

- `make test` green (124 + new units); clippy clean; CI green with
  the DRM build check.
- T-commit ownership visible in code; no `backend::renderer`
  anywhere (grep).
- Refresh/geometry advertised from the real mode (assert in a unit
  test against a mocked mode; verified live in the smoke).
- Headless tick semantics unchanged (suite proves it).
- The interactive checklist delivered; Roland's smoke verdict
  recorded afterward as verification, per the now-established
  pattern.

## Out of scope

Cursor plane and libinput (T2 — winit input does not exist on metal,
and that is expected: this smoke is about pixels and VT survival;
input on metal arrives next task — note it prominently in the
checklist so the silent keyboard isn't read as a bug); frame
scheduler and presentation-time (T3); GPU (T4+); multi-output,
hotplug, suspend/resume (M9); plane offload beyond primary (M5).
