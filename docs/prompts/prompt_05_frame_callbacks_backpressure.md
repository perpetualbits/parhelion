# Prompt 05 — Reverse direction: frame callbacks, flush ownership, backpressure

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-26.
**Milestone:** M1, task T2 (see `docs/plans/m1_tasks.md`).
**Reads first:** `docs/plans/m1_tasks.md` T2; `docs/scene_graph_v1.md`;
spike report §3 Q1 (what is `Send`) — and the chat-project rider this
task exists to discharge: client→scene was proven in M0; scene→client
was not.

---

## Context

Until now, information flows one way: clients → dispatch → scene →
render. Wayland requires the reverse: `wl_surface.frame` callbacks
must fire when the compositor has used a commit, and that decision is
born on the render side. This task builds the reverse path and, while
opening it, installs the backpressure policy — because the moment two
threads feed each other queues, "what happens when one floods" stops
being theoretical.

## Design constraints

1. **All protocol-object interaction stays on the dispatch thread.**
   Even though `DisplayHandle` is `Send + Sync`, v1 does not exercise
   cross-thread event posting: the render side *enqueues* "frame
   presented" notices; the dispatch thread turns them into
   `wl_callback.done` sends and performs the flush. One thread touches
   Wayland objects, period — it is the simplest model that satisfies
   §7, and it keeps the door open to sharding without re-auditing send
   sites. If you find a hard reason this cannot work, stop and report
   (that would be design news).
2. **Waking the dispatcher is the technical crux.** The dispatch
   thread blocks waiting on client sockets; a render-side notice must
   wake it promptly (calloop channel / ping source, or the equivalent
   in the current loop mechanism). Latency here is callback latency —
   keep it one wakeup, no polling sleeps in the delivery path.
3. **Callback semantics v1, documented as v1:** on each render tick,
   callbacks pending for surfaces included in that snapshot fire, with
   the test-controlled tick supplying a deterministic timestamp
   (monotonic ms per protocol). Real vsync pacing and throttling
   policy for occluded surfaces are M2 — leave the doc note.
4. **Backpressure policy** (the I-10 fairness rider), written into
   code and `docs/scene_graph_v1.md`:
   - Both directions bounded: protocol→scene messages and
     render→dispatch notices.
   - Frame-callback state **coalesces** — per surface, pending
     callbacks are a list the protocol already bounds naturally per
     commit; notices per tick collapse to "tick happened, these
     surfaces" rather than unbounded per-event queueing.
   - Per-client accounting at the `ProtocolHost` boundary: a client
     exceeding its request-queue bound gets its socket unscheduled
     (stops being read) until the scene drains — never dropped
     messages, never a stall for shard-mates. Disconnection remains
     the last resort for a client that also fills its kernel socket
     buffer; if you implement it, it is a protocol-visible behavior —
     document it. Keep the policy this simple; resist inventing QoS.

## Task

1. Render→dispatch notice channel + dispatcher wakeup.
2. `wl_surface.frame` request handling: callback registered at
   commit (double-buffered per protocol — pending until the commit
   that carries it), fired per constraint 3, destroyed after `done`.
3. Flush ownership made explicit: one flush site, on the dispatch
   thread, after processing notices; comment names it as the only one.
4. Backpressure per constraint 4; bounds are named constants with
   module-doc reasoning, not magic numbers.
5. Rig tests:
   - Scripted client: attach-less commit with frame request → render
     tick → client receives `done` with the expected deterministic
     timestamp. (This is the milestone's reverse-direction proof.)
   - Two clients, A floods commits+frame requests in a tight loop
     while B behaves: B's callback latency stays at one tick; A's
     socket is throttled, not killed; process memory for queues
     provably bounded (assert on queue lengths/caps, not RSS).
   - Callback lifecycle conformance: no `done` before the carrying
     commit; callback object destroyed after `done`.
6. `docs/scene_graph_v1.md`: new section for the reverse path, flush
   ownership, callback v1 semantics + M2 note, and the backpressure
   policy. Diary (`#core` `#protocol`, likely `#discovery` — dispatcher
   wakeup usually yields one); session summary; `make test` stated.

## Acceptance

- All prior tests green plus the three rig tests above; clippy clean.
- Exactly one flush call site, on the dispatch thread; grep-verifiable.
- No Wayland object touched off the dispatch thread; grep for
  `DisplayHandle` use outside `ProtocolHost` finds nothing.
- Bounds are constants with reasoning; the flooding test fails if the
  bound is removed (state that you verified this by trying it).
- Policy section exists in the scene doc.

## Out of scope

`presentation-time` and real vsync pacing (M2); occlusion-aware
callback throttling (M2, needs damage/visibility from T4); shm (T3);
any change to snapshot semantics.
