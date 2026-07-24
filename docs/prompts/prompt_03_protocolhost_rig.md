# Prompt 03 — ProtocolHost (shards = 1), protocol rig, M0 closure

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-25.
**Milestone:** M0, task 3b — the final M0 task.
**Reads first:** decision log "2026-07-24 — Smithay threading fit" (all
three entries — they are this task's requirements), spike report §3
(Q1 recipe, Q2 experiment) and §5, `CORE-BOUNDARY.md` §7, CLAUDE.md.

---

## Context

The spike proved the §7 dispatch/scene split compiles and runs; the
decision entries commit us to Smithay as protocol frontend at
`shards = 1` behind a shard-count-agnostic `ProtocolHost`. This task
promotes the spike into `crates/core` and the harness: the real
`ProtocolHost` skeleton, the protocol test rig, and the static
assertion regression guards. It completes M0's acceptance list.

## Task

1. **`crates/core`: the `ProtocolHost` skeleton.** Implement the four
   interface requirements from the decision entry, at `shards = 1`:
   - Client→shard assignment happens at accept time inside
     `ProtocolHost` (`ListeningSocket::accept` → `insert_client` for
     external use; the rig injects via socketpair — both paths go
     through the same assignment seam). Nothing outside `ProtocolHost`
     may assume a single `Display`.
   - The dispatch thread owns `Display` + a thin protocol-only
     `State`; everything it tells the rest of the world crosses a
     channel as `Send` tokens (core-assigned `SurfaceId`, not borrowed
     Wayland resources).
   - The receiving side is a deliberately minimal **scene ledger** — a
     struct tracking live surfaces (created / committed / destroyed)
     by `SurfaceId`, inspectable by the harness. It is *not* the scene
     graph; do not add geometry, buffers, stacking, or any type M1
     would have to fight. Its module doc says exactly this.
   - Protocol scope: `wl_compositor` only (surface create / commit /
     destroy), matching the spike. Prefer `smithay::wayland::compositor`
     (the delegate machinery is what the frontend decision points at);
     if its handler structure forces machinery that fights the ledger
     minimalism, falling back to a raw `wayland-server` global is
     acceptable — module-doc the reasoning, diary it, and flag it in
     your report so Roland can judge whether it deserves a log entry.
   - Dispatch loop mechanism: prefer `calloop` (production's
     substrate, and Smithay's); if its integration cost at this scale
     is disproportionate, a poll loop is acceptable for now with a
     diary note — the spike already established this is a mechanism
     choice, not a threading-model question.
   - Dependency pins per the decision entry: `smithay = "=0.7.0"`
     (minimal feature set — record the features chosen and why in
     `Cargo.toml` comments), workspace `Cargo.lock` committed.

2. **Static `Send`/`Sync` regression guards** (spike §5.5): the Q1
   assertions land in `crates/core` as **compile-time** guards
   (`fn assert_send<T: Send>()` style, referenced so they cannot be
   dead-code-eliminated away from the build). Their failure mode is a
   build break on a future Smithay/wayland-server bump — comment them
   with exactly which decision they guard.

3. **`crates/harness`: the protocol rig.** The spike pattern,
   promoted: in-process socketpair, scripted `wayland-client` on its
   own thread, fully deterministic, no external sockets or processes
   (spike §5.2). Shape the API so a test reads as: script the client,
   then assert on both sides — wire behavior (client saw the globals,
   roundtrips complete) and ledger state (surface exists after
   create+commit; gone after destroy; client disconnect cleans up).
   First tests: exactly those three, plus a two-client test proving
   ledger attribution doesn't confuse clients (two clients is still
   one shard — that's the point of the seam).

4. **M0 closure.** Walk the milestone plan's M0 acceptance list item
   by item and state each result explicitly in the session summary.
   Add a status line to the plan's M0 section: `**Status: complete
   YYYY-MM-DD** (sessions: …)` — the plan is a re-entrant document;
   its state should be readable from the document itself. If any item
   is not green, stop and report instead of declaring completion.

5. **Close per discipline:** session summary; diary (`#core`,
   `#harness`, plus earned tags); project index for any new doc (none
   is expected — `ProtocolHost` is governed by CORE-BOUNDARY §3/§7
   until it earns its own document, per the subsystem table); `make
   test` result stated.

## Acceptance

- `make test` green: existing 13 + the new protocol-rig tests; clippy
  `-D warnings` clean; static guards compile (and are provably part of
  the build, not dead code).
- The four `ProtocolHost` interface requirements are each visible in
  the code and named in comments — a reviewer can point at where each
  one lives.
- Ledger is inspectable by tests and contains nothing M1 will fight.
- Smithay pinned `=0.7.0`, lockfile committed, features documented.
- Milestone plan M0 marked complete with the item-by-item results in
  the session summary — or an honest report of what blocked it.

## Out of scope (explicitly)

`xdg-shell`, `wl_shm`/buffers, input, frame callbacks, the renderer
seam, scene-graph types (all M1). Shards > 1 (the seam exists; the
escalation trigger is measured contention, per the decision entry).
The scene→client reverse-direction proof (frame callbacks originating
scene-side) — that is M1's first protocol task, already flagged in
the chat project's spike review.
