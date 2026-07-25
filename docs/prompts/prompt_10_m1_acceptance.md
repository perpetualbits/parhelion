# Prompt 10 — The acceptance run: a real terminal, and M1 closure

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M1, task T7 — the final M1 task.
**Reads first:** `docs/plans/m1_tasks.md` T7 + standing notes;
`docs/parhelion_milestone_plan.md` M1 acceptance list; the T6 session
summary (the `.lock` leftover and the untested-window note both come
due here).

---

## Context

Everything M1 built exists so that a terminal nobody wrote for us can
connect, map, render, and echo keystrokes. T7 proves it twice: once
**automated and headless** (the strategic move — the M1 acceptance
becomes a CI test, not an anecdote), once **interactive** under the
winit window with Roland's eyes. Then the milestone closes item by
item.

## The client

**foot** is the primary target: shm-rendered, minimal, packaged in
Ubuntu (CI gains `apt install foot` — amend the CI comment's
dependency ledger accordingly). `weston-terminal` is the documented
fallback if foot hard-requires something out of M1's world.

## Pre-authorized scope vs. stop-and-report

A real client will bind globals we have not needed yet. The line:

- **Pre-authorized, in-scope now:** `wl_output` — a real client
  realistically requires it. Implement properly, not as a stub: one
  output, fixed mode matching the backend size, scale 1, geometry
  events per protocol, enter/leave on surfaces. Rig tests included.
- **Everything else discovered missing:** apply the standing rule. If
  foot (or the fallback) refuses to run without an interface —
  subcompositor, decoration negotiation, data-device, a popup — **stop
  and report** with the exact refusal evidence. Do not stub protocols:
  an advertised-but-hollow global is a lie to every future client. If
  the client runs but degrades without one (logs a warning, skips a
  feature), note it in the report and continue — absence is honest.

## Task

1. **Graceful shutdown for `parhelion-dev`** (the T6 leftover): handle
   SIGTERM/SIGINT, remove socket + `.lock`, exit clean. Test the
   removal headlessly (spawn, signal, assert files gone).
2. **`wl_output`** per the pre-authorization.
3. **The automated acceptance test** (the centerpiece — a harness
   integration test, CI-marked if runtime is significant):
   - Serve a real socket headlessly (T6's split made the binary's
     plumbing display-free; use it or the harness equivalent).
   - Spawn foot as a subprocess against it (skip-with-loud-message if
     foot is absent locally; present in CI).
   - Assert the full arc: client binds globals → xdg toplevel maps →
     shm buffers commit → frame callbacks flow (foot throttles on
     them; a stalled callback loop is the likeliest failure and this
     test is its tripwire).
   - Inject typing through the funnel (a printable sentence plus
     Enter). Assert with counters and pixels, not goldens (fonts make
     terminal output machine-dependent): the typed frames' damage is
     a small fraction of the output (state the bound as a named
     constant with reasoning); pixels outside the damage region are
     byte-identical across a typing frame; pixels inside changed.
     **This is the founding thesis measured: typing redraws a region,
     not a frame.**
   - Terminate foot; assert unmap + cleanup (no leaked scene nodes,
     counters sane).
4. **Conformance sweep:** for each implemented global, verify an
   error-path/conformance test exists; fill trivial gaps in this
   session, list any non-trivial gap in the report rather than
   growing scope.
5. **Interactive protocol** (Roland's part, scripted by you): exact
   commands for the two-terminal smoke — `parhelion-dev`, then
   `WAYLAND_DISPLAY=… foot` — and a short checklist of what to look
   at (window appears at cascade origin, glyphs render, typing echoes
   with no visible lag, resize survives, Ctrl-C in foot exits clean,
   dev binary SIGINT leaves no socket litter). State plainly what you
   verified headlessly vs. what needs eyes.
6. **M1 closure:** walk the milestone plan's M1 acceptance list item
   by item in the session summary with evidence pointers (test names,
   counter values); add the `**Status: complete**` line to the plan's
   M1 section — or the honest blocker report instead. Update
   `docs/plans/m1_tasks.md` T7 status.
7. **Close per discipline:** docs touched where behavior grew
   (`wl_output` section in the scene doc), decision log if anything
   rose to load-bearing, diary (`#milestone` — this one deserves a
   narrative entry: what M1 taught), session summary, map per the
   standing order — `wl-clients`/"Real terminal client" → done, M1
   nodes to earned statuses, `updated` bumped, `node --check` clean.

## Acceptance

- `make test` green including the automated acceptance test; clippy
  clean; CI green with foot installed.
- The acceptance test fails when frame callbacks are sabotaged
  (verify by temporarily breaking the notice path, then revert —
  state it; a terminal that renders once and freezes must not pass).
- Damage proportionality asserted with the named bound; outside-damage
  byte-identity asserted.
- No protocol stubbed; any stop-and-report delivered with evidence.
- M1 marked complete with the item-by-item walk, or blockers reported.
- Interactive checklist delivered for Roland's smoke.

## Out of scope

Fixing anything the interactive smoke reveals beyond the checklist's
scope (report, we slice); performance work (M5 owns benchmarks — the
proportionality bound here is a correctness claim, not a speed claim);
decoration protocols; popups (standing note: report if demanded);
M2's world (vsync, dmabuf, DRM, cursor plane, libinput).
