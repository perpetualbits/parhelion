# Prompt 01 — Spike: Smithay threading fit

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M0, task 2 (see `docs/parhelion_milestone_plan.md`).
**Kind:** investigation spike. The deliverable is a *report and a
recommendation*, not production code. The decision itself is Roland's.

---

## Context

`CORE-BOUNDARY.md` §7 specifies Parhelion's threading model: T-input,
T-commit, T-render, sharded protocol dispatch threads T-proto[n], and
workers — each resource owned by exactly one thread, communication by
message passing and immutable scene snapshots. Smithay, our intended
protocol foundation, is designed around calloop with a strong
single-event-loop, single-`State` idiom (see anvil). The open question
(decision log, "Pending"): **can Smithay be driven within our
threading model, and at which layer do we consume it?**

This verdict shapes the `crates/core/` internals and the M0 headless
backend (task 3), which is why it runs before any real code.

## Questions to answer (with evidence, not opinion)

1. **Protocol dispatch threading.** In current wayland-rs (the version
   current Smithay uses — record both version pins in the report): can
   client dispatch be sharded across threads with per-client ordering
   preserved? Concretely: what is `Send`/`Sync` and what is not among
   `Display`, `DisplayHandle`, client/backend types; can multiple
   event loops each own a disjoint subset of client connections; where
   does the global `State` coupling bite?

2. **If sharding is not possible (likely):** is single-threaded
   protocol dispatch acceptable under our invariants? Note carefully:
   protocol dispatch is *not* the frame path — I-1 protects
   render/commit, and §7 already separates T-render/T-commit from
   dispatch. The real question is whether Smithay's `State` idiom
   forces protocol state and scene/render state into **one** thread,
   or whether a dispatch-thread-owned protocol state can publish
   changes to a separately-owned scene via messages. Build the
   smallest experiment that proves this split compiles and runs: a
   dispatch thread owning Smithay state, a second thread owning a toy
   "scene," messages between them, one scripted client connecting.

3. **Layer selection.** Independent of dispatch: which Smithay layers
   do we want regardless of the verdict — backends (DRM/KMS, libinput,
   winit, headless), renderer abstractions (we will have our own
   renderer; do Smithay's traits help or constrain?), utilities
   (serials, geometry types)? For each: consume, consume-with-wrapper,
   or bypass, with one line of reasoning.

4. **Evolution risk.** Smithay's release cadence and API churn:
   what does pinning look like, how painful have recent major
   upgrades been for downstream compositors (cosmic-comp is the
   natural reference), and does anything in its roadmap touch the
   threading question?

## Method and constraints

- Timebox the experiment: the smallest programs that produce evidence.
  Spike code lives in `tools/spikes/smithay-threading/` (committed —
  spikes are reference material, exempt from production comment
  density but still headed by a block comment stating what the
  experiment demonstrates). It is not a workspace member of the
  production build if that complicates lints; a standalone crate in
  the spike directory is fine.
- No changes to `crates/*` in this session.
- Acceptable outcomes include "shards=1 now, interface allows sharding
  later" — CORE-BOUNDARY §7 describes ownership, not a mandatory
  shard count. If that is your recommendation, state what the
  interface must look like so shard count is an implementation detail
  rather than an architectural commitment.
- If the investigation surfaces a genuine conflict with CORE-BOUNDARY
  §7 (not just "shards=1"), stop and report — that is a design
  question for Roland, per CLAUDE.md.

## Deliverables

1. `docs/smithay_threading_spike.md` — the report: version pins,
   findings per question with evidence (code references, compiler
   errors are evidence too), a recommendation with tradeoffs, and a
   closing section "implications for M0 task 3 (headless backend +
   harness)".
2. The spike experiment under `tools/spikes/smithay-threading/`,
   runnable via a documented one-line command.
3. Project index entry for the report; session summary; diary entry
   (this session will contain surprises — that is what spikes are
   for; tag `#discovery` liberally).
4. **A drafted decision-log entry at the end of the report, not yet
   appended to the log.** Roland reads the report, confirms or amends
   the recommendation, and only then does the entry land (that
   append is a "small, clearly-scoped change" — no separate session
   needed).

## Acceptance

- Every numbered question answered with evidence; speculation labeled
  as speculation (CLAUDE.md, technical attitude).
- The split experiment from question 2 compiles and runs, or the
  report explains precisely why it cannot.
- Recommendation stated plainly enough that Roland can say yes/no in
  one sentence.
- `make test` still green (spike must not break the workspace).
- Summary, diary, index updated; decision entry drafted but not
  appended.
