# Prompt 02 — Headless backend, golden-test rig, CI skeleton

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M0, task 3a (task 3b — ProtocolHost + protocol rig — is prompt 03).
**Reads first:** `docs/smithay_threading_spike.md` §5 (implications for this task), `docs/parhelion_milestone_plan.md` M0, CLAUDE.md.

---

## Step 0 — Land the spike decision (Roland's confirmation, conveyed by this prompt)

Roland confirms the spike recommendation as stated. Append the drafted
entry from `docs/smithay_threading_spike.md` §8 to
`docs/parhelion_decision_log.md` verbatim, and strike the matching
"Pending" item. Small, clearly-scoped change; do it before anything
else so this session's work sits on a settled decision.

## Context

Per spike §5.1, Smithay is not on this task's critical path at all:
M0's headless rendering is ours, and the protocol frontend arrives in
prompt 03. This task builds the thing the milestone plan calls the
project's velocity multiplier: a headless frame producer, a
golden-image comparator that provably can fail, and the CI that runs
them on every push.

## Task

1. **`crates/backend-headless`** — the smallest honest frame producer:
   - A `Frame` representation: width, height, tightly-packed RGBA8
     pixels. Deterministic by construction — no time, no randomness,
     no float paths that vary by architecture.
   - A test-pattern renderer (plain CPU code) producing a pattern that
     deliberately exercises a comparator: smooth gradients (tolerance
     territory), hard edges and a pixel grid (off-by-one territory),
     distinct corner markers (orientation/stride bugs), and a solid
     reference patch of known exact color.
   - **Do not design the renderer architecture.** No traits
     anticipating M1's renderer, no scene-graph types, no GPU. The
     interface is only what the harness needs: "give me frame N of
     size W×H." M1 replaces the internals; the seam it needs will be
     designed then, with the scene graph on the table (CLAUDE.md: no
     abstractions beyond what the task requires).

2. **`crates/harness` — the golden rig:**
   - Golden storage: PNG files under `crates/harness/goldens/`,
     committed. Pick the minimal PNG crate and record the choice
     (one-line reasoning in the module doc; no decision-log entry
     needed unless you consider it load-bearing).
   - Comparator with an explicit policy: per-channel tolerance T plus
     a max-differing-pixel budget P, both defaulting to **0** —
     tolerance is loosened per-test with a stated reason, never
     globally. (CPU-rendered patterns should match exactly; the
     tolerance machinery exists for the GPU future, not for today.)
   - On mismatch: write the actual frame and a visual diff image
     (differing pixels highlighted) to `target/golden-failures/…`,
     and fail with a message naming all three paths.
   - Blessing workflow: `UPDATE_GOLDENS=1 make test` (or equivalent)
     rewrites goldens for failing tests; document it prominently —
     this is the command Roland will actually use.
   - **Prove it can fail** (milestone acceptance): a meta-test that
     feeds the comparator a deliberately perturbed frame (one changed
     pixel; and a shifted-by-one variant) and asserts the comparison
     *reports failure with the diff artifacts*. The meta-test itself
     passes — CI stays green while failure detection is proven.
   - First real golden test: headless test pattern vs committed
     golden.

3. **CI skeleton** — GitHub Actions workflow (`.github/workflows/`):
   checkout, stable Rust toolchain, cargo cache, `make test`. Per
   spike §1, no system Wayland packages are needed (pure-Rust backend;
   and this task uses no Smithay at all) — keep the runner
   dependency-free and note in a comment why no apt-install step
   exists. The workflow file lands in this commit; its first run
   happens whenever Roland pushes (you do not push).

4. **`docs/harness_design.md`** — short canonical doc for the harness
   subsystem: frame/golden format, comparator policy and the
   tolerance-is-per-test rule, blessing workflow, determinism
   requirements for anything that wants a golden test, failure
   artifact locations. Update CLAUDE.md's subsystem table cell for the
   harness (small scoped change) and add the doc to the project index.

5. **Close per discipline:** session summary; diary entry (`#harness`
   plus whatever the session earns); `make test` result stated
   explicitly.

## Acceptance

- Step 0 landed: decision log carries the spike entries, Pending item
  struck.
- `make test` green locally, including the golden test and the
  prove-it-can-fail meta-test.
- Deleting a golden and running the blessing workflow regenerates it
  byte-identically (determinism check — state that you verified this).
- CI workflow file present and self-contained; expected to be green on
  Roland's next push with no runner setup.
- `docs/harness_design.md` exists, indexed; CLAUDE.md table updated.
- No renderer architecture invented; no Smithay dependency introduced;
  no scene-graph types created.

## Out of scope (explicitly)

`ProtocolHost`, the protocol rig, the wl_compositor round-trip, and
the static `Send`/`Sync` assertion migration — all prompt 03. Anything
touching M1's renderer seam. Multi-frame animation testing and
dt-trace replay (M3 territory; the golden rig will grow it then).
