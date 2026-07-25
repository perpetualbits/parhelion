# Prompt 11 — Clipboard v1, advertise-what-we-honour, and closing M1

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M1, task T7b (unblocking T7's centerpiece).
**Reads first:** the T7 session summary (blocker evidence + the
subcompositor finding); `CORE-BOUNDARY.md` I-7; `docs/plans/m1_tasks.md`.

---

## Step 0 — The decision, conveyed

Roland decides **option (a)**: implement `wl_data_device_manager`
properly. Append the decision-log entries (resolving T7's Pending
item):

1. **Clipboard v1 = core-protocol selection semantics, focus-gated.**
   Offers flow only to the keyboard-focused client — the protocol's
   own shape is the v1 capability model and satisfies I-7's letter.
   The deeper capability design (security-context restrictions on
   selection access, clipboard-manager protocols, primary selection)
   is deferred to M4's capability work — recorded deferral, with a
   pointer note added to the capability section of the dialect spec's
   §8 territory when M4 slices.
2. **Advertise only what we honour.** A global we advertise but do not
   implement is a standing lie to clients; `wl_subcompositor` is
   removed from advertisement until subsurfaces are real (likely
   sliced alongside popups, milestone TBD). Same principle that
   refused the stub, applied to the pre-existing case.

## Task

1. **`wl_data_device_manager` via Smithay's data-device machinery**
   (state, delegates, selection handler). Scope:
   - **Selection (clipboard) fully works:** set_selection honoured
     (with Smithay's serial discipline), offers delivered on keyboard
     focus enter and on selection change to the focused client,
     `receive` transfers bytes through the pipe, source `cancelled`
     on replacement, client death clears its selection.
   - **Drag-and-drop is honestly deferred, not hollow:** handle
     `start_drag` by immediately cancelling the source (protocol-legal
     compositor behavior), module-doc'd as v1 policy, map seam node
     for DnD, and a rig test proving the client receives `cancelled`
     rather than silence. If Smithay's machinery makes real DnD
     nearly free *without* touching our pointer routing, you may say
     so in the report — but do not implement it this session; grabs
     meeting our focus model is its own design conversation.
   - Rig tests: two clients — A copies (`text/plain`), focus moves to
     B, B receives the offer and reads the exact bytes; unfocused C
     receives nothing (the focus gate asserted, not assumed);
     selection replacement cancels A's source; A's death clears the
     selection without disturbing B.
2. **Subcompositor honesty:** determine whether the `wl_subcompositor`
   global is separable from Smithay's compositor delegate. If yes:
   stop advertising it; rig-assert the registry no longer lists it.
   If the delegates are welded: stop and report with evidence — do
   not ship the lie another session, and do not half-implement
   subsurfaces to launder it.
3. **The acceptance test, completed:** re-run T7's centerpiece with
   foot — now expected to pass registry and reach the full arc (map,
   shm commits, frame callbacks, typing with the damage-proportionality
   and outside-damage byte-identity assertions, teardown). The
   frame-callback sabotage check from prompt 10 applies now that the
   test can run.
4. **M1 closure, second attempt:** the item-by-item walk with the
   ninth item green; `**Status: complete**` in the milestone plan;
   `m1_tasks.md` T7 updated — or, if anything still blocks, the
   honest report again (the pattern is now well-established).
5. **Interactive checklist, extended:** the T7 checklist plus a
   clipboard smoke — copy in foot, paste into a second foot (or
   `wl-paste` if you have wl-clipboard available headlessly, in which
   case automate it and say so). Restate the two items only Roland's
   eyes can settle (resize, cursor-over-window).
6. **Close per discipline:** scene/protocol doc sections for the data
   device (semantics, the DnD deferral, the M4 pointer); diary — this
   session's narrative is "the compositor learned to refuse honestly
   twice"; session summary; map per standing order (data-device node
   done with a DnD seam part; subcompositor advertisement removal
   reflected; "Real terminal client" → done if item 3 passes;
   `updated` bumped; `node --check` clean).

## Acceptance

- `make test` green including the completed foot acceptance test and
  the new data-device rig suite; clippy clean; CI green.
- The focus gate is asserted by a test (client C), not inferred.
- `start_drag` produces `cancelled`, tested.
- Registry no longer advertises `wl_subcompositor` (or the welded
  report is delivered).
- M1 marked complete with the full walk, or blockers reported.
- No new global advertised beyond `wl_data_device_manager`.

## Out of scope

Real drag-and-drop (own design conversation — grabs vs. our focus
model); primary selection (`zwp_primary_selection`); clipboard
managers and `ext-data-control`; subsurface implementation; anything
M2. The capability refinement of selection access is M4's, per the
decision above.
