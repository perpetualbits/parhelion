# Session summary — 2026-07-25 — M2 T0: debt & honesty

**Task:** M2 T0 (prompt 12, revised) — index the M2 plan, land the superseding
advertise/refuse decision, build the subcompositor tripwire, pay T2's
fd-deregistration promise via the per-client-readiness restructure, sweep the
ledger, close per discipline.

**Build/test result:** `make test` — **110 tests green** (up from T7b's 108, +2),
clippy clean, zero warnings. The foot acceptance test and the whole clipboard
suite are green on the new client-intake path.

---

## 1. The tripwire: not built, because its premise was my error

**T0's first item cannot be done, and the reason is a measurement I got wrong.**

T7b reported — and prompt 12's superseding decision repeats — that `foot` "binds
`wl_subcompositor`, calls `get_subsurface` **zero** times". That was a bad grep:
`wl_subcompositor@N.get_subsurface` where `WAYLAND_DEBUG` prints `#`. It matched
nothing, and I read empty output as evidence of absence.

**What is actually true:** foot creates **nine** subsurfaces during startup and
attaches buffers to **eight** of them. They are its client-side decorations.

Both candidate refusal points were implemented and measured before reporting:

| Refuse at | Result |
|---|---|
| `get_subsurface` | foot dies at startup (nine subsurfaces) |
| a subsurface **committing a buffer** — the narrower, better rule | foot dies moments later (eight carry pixels) |

The second is worth noting because it *nearly* worked: foot's first subsurface is
pixel-less (an input region for its resize border), and refusing only where
content would actually be dropped would have spared it. Eight buffered
subsurfaces later, it does not.

**There is no refusal point that keeps an honest client alive.** Loud refusal and
a working terminal are mutually exclusive until subsurfaces exist, and M1's
acceptance criterion *is* the terminal. So:

- the tripwire is **not implemented**; the code and the scene doc (§12.3) carry
  the measurement and the reason;
- the decision log has a **CORRECTION** entry naming what it overturns, and the
  T7b summary's claim is struck in place with a pointer to it;
- the conformance suite pins the current, unhappy behaviour
  (`a_subsurface_is_accepted_and_its_content_is_silently_not_composited`) so the
  test **inverts** the day T7 lands;
- Pending re-raised: subsurfaces, scheduled M2 T7.

**Consequence you will see:** foot renders **without decorations** under
Parhelion. That is this debt, visible — not a new bug. It was true before this
session too; nobody had looked.

**The principle itself is untouched** and still governs any future global: it
simply has no applicable instance today, because its one instance turned out to be
a global that clients both require *and* use.

## 2. The per-client-readiness restructure (T2's promise, paid)

`ProtocolHost`'s intake changed shape:

- at `add_client`, the client's socket is **`try_clone`d** and *that* descriptor is
  registered as its own `calloop` source;
- readiness dispatches **that client alone** (`dispatch_single_client`);
- wayland-backend's aggregate poll fd is **no longer watched** — grep-verifiable:
  there is no `Generic::new(display, …)` anywhere;
- throttling is therefore literal: `handle.disable(&token)`. No readiness, no
  wakeups, no spin;
- re-arm below `RESUME_PENDING_FRAME_CALLBACKS` (16 = a quarter of the throttle
  mark). Hysteresis, so a steady flooder does not toggle its registration every
  tick. The re-arm lives in the **present/callback path**, because a disabled
  client can never re-arm from its own dispatch.

**The spin ends by construction, and the numbers say so.** Reproducing the old
semantics exactly (skip the throttled client, leave its source enabled) makes the
dispatch loop turn **100 046 times in 300 ms**; the fixed build turns ~15 against a
bound of 200. Sabotage stated and reverted, as required.

**Rejected alternatives, recorded in §8.5 so they stay rejected:** edge-triggering
the aggregate fd (ends the spin, starves shard-mates — the fd never goes quiet
while a throttled client holds data, so nobody else's readiness produces an edge);
timer-based rate limiting (bounds the spin without ending it — it would have made
the iteration assertion pass while the promise stayed unkept).

**Side effect worth having:** per-client sources are the shape a *shard* owns. The
restructure moves toward the spike's shard-count-agnostic interface rather than
bending it.

**Regression care** (this touches every client's front door): the two-client
fairness test, connect/disconnect churn, the whole clipboard suite, and the foot
acceptance test are green. Two wires had to be re-attached during the move — the
deferred clipboard-liveness refresh (T7b's fix) lost its trigger when
`pump_display` went away, and the throttle re-arm needed a home in the present
path. Both are now tested where they live.

## 3. Ledger sweep

| M1 note stamped "M2" | Disposition |
|---|---|
| fd-deregistration backpressure (§8.5) | **Paid this session** |
| Occlusion-aware callback throttling (§8.3) | Waits — needs T3's pacing, per the prompt |
| Fire-every-tick callback semantics (§8.3) | Waits — comes due at T3 |
| libinput / real T-input (§11.2) | T2 |
| Cursor plane, client cursor surfaces (§11.4) | T2 |
| Buffer scale/transform coordinate merge (§9.3) | Waits — a real feature, T2/T3 territory |
| Output refresh rate is a claim without a vblank (§12.1) | T1/T3, with the real connector mode |
| Cascade cannot clamp to the output (§10.4) | Waits — the output size *is* known now (T7), but placement is S1's policy in M4, and changing it would re-bless goldens for no user-visible gain |

## Files changed

### Modified — core
- `crates/core/src/protocol.rs` — the restructure (`ClientSource`,
  `State::display`, `admit_client`, `dispatch_one_client`, `update_throttles`,
  `remove_client_source`, `republish_backlog`), `RESUME_PENDING_FRAME_CALLBACKS`,
  the dispatch-iteration counter, and the corrected `wl_subcompositor` note at the
  global's creation. `WL_DISPLAY_ERROR_IMPLEMENTATION` is kept (documented,
  unused) for T7's inversion and any future advertise-before-support case.

### Modified — harness
- `crates/harness/src/protocol_rig.rs` — `get_subsurface`, `flush_best_effort`
  (a full socket is backpressure arriving, not a failure).
- `crates/harness/tests/protocol.rs` — `a_sustained_flood_does_not_spin_the_dispatch_loop`
  (no-spin + re-arm assertions).
- `crates/harness/tests/conformance.rs` — the subsurface behaviour pinned, ready to
  invert at T7.

### Modified — docs
- `docs/scene_graph_v1.md` — §8.5 rewritten (the delivered design, hysteresis,
  rejected alternatives, shard-readiness); §12.3 rewritten (the corrected
  subsurface story).
- `docs/parhelion_decision_log.md` — the superseding advertise/refuse entry; the
  **CORRECTION** entry; the intake-restructure entry; Pending re-raised for
  subsurfaces.
- `docs/sessions/_session_2026-07-25_clipboard-t7b.md` — the wrong claim struck in
  place.
- `docs/diary.md` — "the measurement I got wrong, and the spin that ended".
- `docs/parhelion_project_index.md` — M2 plan indexed; current state.

### Project map
- `subsurfaces` node: description corrected to the measured truth, still `seam` →
  T7; `protocol-host` gained a per-client-readiness part; `updated` bumped;
  `node --check` clean.

## For Roland

1. **The superseding decision in prompt 12 rests on my bad number.** Its principle
   is fine and I have landed it; its stated application (`wl_subcompositor`) is
   not achievable. If you want the entry reworded now that the premise is gone, say
   so — I have left it standing with the correction directly beneath it, because
   the principle is worth keeping for the next global.
2. **foot has no decorations** under Parhelion, and will not until T7. Worth
   knowing before the interactive smoke.
3. Nothing else in T0 is outstanding.
