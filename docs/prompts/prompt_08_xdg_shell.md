# Prompt 08 — xdg-shell minimal

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M1, task T5 (see `docs/plans/m1_tasks.md`).
**Reads first:** `docs/plans/m1_tasks.md` T5; `docs/scene_graph_v1.md`;
CORE-BOUNDARY C10 (core fallbacks — placement lives there until M4).

---

## Context

Real applications do not commit bare `wl_surface`s; they speak
xdg-shell. This task implements the minimal toplevel lifecycle — and
with it comes a semantic reckoning we have deferred since T1:

**The mapping-semantics migration (the headline).** Per Wayland, a
surface without a role is never displayed. Until now our scene has
composited any committed surface — convenient for tests, but
non-conformant, and T5 ends it: only mapped toplevels (and the
harness's scene-injected solid nodes, which bypass protocol entirely
and stay as they are) become visible scene content. Client-driven
tests from T3/T4 migrate to a harness helper that performs the full
dance — create surface → get xdg_surface → get toplevel → initial
commit → receive configure → ack → commit with buffer → mapped. One
helper, every test readable.

**Explicitly preserved:** the T2 frame-callback semantics ("every tick
fires all committed pending callbacks, not visibility-gated") stay
exactly as documented — the attach-less callback proof test keeps its
meaning on what is now an unmapped surface. Occlusion/visibility
gating remains M2. If the migration would force a change to T2's
tests, stop and report; that is a semantics question, not plumbing.

## Design constraints

1. **Smithay frontend layer:** `smithay::wayland::shell::xdg`
   (`XdgShellState`, handler, delegate). Seam vigilance continues: no
   `smithay::backend::renderer`, no `smithay::desktop` — if the xdg
   machinery reaches for either, stop and report (the layer table
   predicts it will not).
2. **Lifecycle per protocol, strictly:**
   - Initial commit without buffer → compositor sends `configure`
     (size 0×0 — client decides; states empty in v1); client must
     `ack_configure` before committing a buffer.
   - Buffer committed before the initial configure is acked →
     protocol error, per spec. Invalid ack serial → protocol error.
   - Second role on a surface → protocol error.
   - `xdg_wm_base` ping/pong implemented (trivial, and conformance
     suites poke it).
   - Null-attach commit or toplevel destroy → unmap → scene node
     removed with structural damage (T4's unmap path already does the
     damage part — assert it fires).
3. **Placement is C10 fallback, loudly temporary:** deterministic
   cascade (fixed origin + per-toplevel offset, named constants,
   module doc saying "policy daemon replaces this in M4"). Determinism
   matters — goldens depend on it.
4. **Scene state grows role metadata:** title and app_id captured and
   inspectable by the rig (future policy/debug food); no behavior
   hangs off them yet.
5. **The rig learns to assert protocol errors:** the error cases above
   require the harness to observe "client received a protocol error /
   was disconnected with the expected error code" deterministically.
   Build that capability once, cleanly — it will be reused forever.

## Task

1. Xdg state + handler wiring in `ProtocolHost`; configure/ack serial
   tracking; ping/pong.
2. Map/unmap → scene messages (mapped toplevels become nodes at their
   C10 placement; unmap removes with damage).
3. Title/app_id → scene metadata.
4. Harness: `map_toplevel`-style helper (and a variant allowing a
   custom draw between configure and mapped-commit); protocol-error
   assertion capability.
5. Test migration: T3/T4 client-driven tests move to the helper;
   scene-injected solid-node tests untouched; T2 callback tests
   untouched in meaning (see above).
6. New tests:
   - Conformance: the configure/ack dance happy path;
     buffer-before-ack error; bad ack serial error; double-role
     error; ping/pong.
   - A scripted xdg client reaches mapped and composites (golden via
     the helper — this replaces/subsumes the raw-commit golden path).
   - Unmap-on-destroy and unmap-on-null-attach reflected in scene
     state and damage counters.
   - Two toplevels cascade deterministically (golden shows both,
     offset visible).
7. Docs: `scene_graph_v1.md` — mapping semantics section (the
   role/visibility rule, what "mapped" means here, the preserved T2
   callback note); decision-log entry for the mapping-semantics
   migration (it is load-bearing — it changes what "in the scene"
   means); diary; session summary; `make test` stated.

## Acceptance

- All tests green post-migration; clippy clean; goldens re-blessed
  only where the helper's dance legitimately changes setup (list each
  re-blessed golden and why in the session summary — re-blessing
  without stated cause is the one golden-rig sin).
- Every protocol-error test asserts the specific error, not just
  disconnection.
- Grep: no `smithay::desktop`, no `smithay::backend::renderer`.
- T2's callback tests pass unmodified (or the stop-and-report fired).
- Cascade constants named, documented as C10-temporary.

## Out of scope

Popups and positioners (post-M1 unless T7's terminal demands one —
the standing note's stop-and-report applies); interactive move/resize;
maximize/fullscreen/minimize states; server-side decorations;
min/max size hints; wm capabilities advertisement beyond defaults;
input and focus (T6 — mapped toplevels exist but nothing focuses them
yet).
