# Prompt 12 — M2 opens: debt & honesty

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-25.
**Milestone:** M2, task T0 (see `docs/plans/m2_tasks.md` — install it
from `downloads/` as step 0 and index it).
**Reads first:** `docs/plans/m2_tasks.md` (whole — it is the
milestone's shape); the T7b session summary; the T2 session's
flood-spin note; decision log "advertise only what we honour".

---

## Step 0 — Verify the M2 plan; land the superseding decision

`docs/plans/m2_tasks.md` is already installed (T7b session); verify,
index it if not yet indexed. Then append the superseding decision
entry (Roland's resolution of the T7b Pending item, conveyed by this
prompt):

> **Advertise-before-support requires loud refusal at point of use —
> never silent wrongness.** Supersedes "advertise only what we
> honour" in its absolutist reading, which T7b proved conflicts with
> reality: clients hard-gate on the *presence* of globals they never
> *use* (foot binds `wl_subcompositor`, calls `get_subsurface` zero
> times, refuses to start without the global). Withdrawal fails
> honest clients at the door; silent non-support renders wrong.
> Resolution: a global may be advertised ahead of support only if
> every unsupported request on it posts a protocol error with a clear
> message. **Known cost, accepted:** this converts degraded into dead
> for clients that actually create subsurfaces (toolkits use them for
> tooltips, popups, CSD shadows) — the right trade for a development
> compositor, and temporary: retired when subsurfaces land (M2 T7).
> Applied to `wl_subcompositor` now. Source: prompt 12 / chat review
> of T7b. Affects: protocol layer, T7b's pinned advertised-globals
> test.

Strike the Pending item.

## Task

1. **The subcompositor tripwire.** `get_subsurface` posts a protocol
   error (pick the honest code; a display implementation error with
   message text naming the situation — "subsurfaces not yet
   implemented; tracked for M2" — is acceptable if no better fits).
   Rig test: a client calling `get_subsurface` dies with that
   specific error; foot still runs (the acceptance test is the
   regression guard). Map: subsurface part marked as seam → T7.
2. **fd-deregistration backpressure — via the per-client-readiness
   restructure** (the T2 promise, mechanism corrected per your own
   review: the aggregated-epoll shape has no per-client source to
   remove, so the shape changes). Restructure `ProtocolHost`'s intake:
   at `add_client`, `try_clone` the client's `UnixStream` and register
   a **per-client calloop source** for readiness; on readiness,
   dispatch that client alone (`dispatch_single_client`-class path);
   stop watching wayland-backend's aggregate fd entirely. Throttling
   is then literally "disable/remove the client's source"; re-arm when
   the scene drains below the re-arm mark (hysteresis — named
   constants, reasoning); the spin ends **by construction**, not by
   bounding.
   - **Record the rejected alternatives in the doc section** so they
     stay rejected: edge-triggering the aggregate fd (starves
     shard-mates), timer-based rate limiting (bounds the spin without
     ending it — would pass the iteration assertion while keeping the
     promise unkept).
   - **Decision-log entry required:** this changes `ProtocolHost`'s
     internal shape. Note in it that per-client sources are the
     shard-ready form — the objects a future shard takes ownership
     of — reinforcing the spike's interface requirement rather than
     bending it.
   - Add the dispatch-loop iteration counter and extend the flooding
     test: iterations bounded during a sustained flood. Verify the
     assertion fails against the pre-restructure behavior
     (sabotage-style: state it, revert).
   - Regression care: client connect/disconnect churn, the two-client
     fairness test, and the foot acceptance test all green on the new
     intake path — this touches every client's front door.
3. **Ledger sweep:** grep the diary/doc notes for any other M1 item
   stamped "M2" that is small enough to pay here rather than at its
   scheduled task; pay it or state why it waits. (Known residents:
   occlusion-aware throttling waits for T3 pacing; the fire-every-tick
   callback note comes due at T3 — do not pull those forward.)
4. **Close per discipline:** doc touches (backpressure section
   updated with the deregistration design; scene doc's subsurface
   note), diary, session summary, map per standing order, `make
   test` stated.

## Acceptance

- All prior tests green (including foot acceptance — both the
  tripwire and the new intake path must not disturb it) plus the
  loud-refusal test and the no-spin assertion; clippy clean.
- The no-spin assertion demonstrably fails against the pre-restructure
  behavior (stated, reverted).
- Per-client sources visible in code; the aggregate fd is no longer
  watched (grep-verifiable); the restructure's decision entry landed.
- Hysteresis constants named with reasoning; re-arm tested (flood
  ends → client resumes service, asserted).
- Superseding entry landed with the known-cost sentence; Pending
  struck; map current.

## Out of scope

Everything T1+ (DRM, libinput, scheduler, GPU, dmabuf, subsurfaces
themselves); any new global; any renderer change.
