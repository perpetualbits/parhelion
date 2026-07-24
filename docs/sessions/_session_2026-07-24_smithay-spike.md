# Session summary — 2026-07-24 — Smithay threading spike (M0 task 2)

**Prompt:** `docs/prompts/prompt_01_smithay_spike.md`
**Kind:** investigation spike. Deliverable = report + recommendation + a
decision-log entry *drafted but not appended*. No `crates/*` changes.

## What was done

Stood up a standalone spike crate against real pinned dependencies (smithay
0.7.0 and the wayland-rs 0.31 stack), gathered evidence three ways — static
`Send`/`Sync` assertions (compiler as oracle), source reading of the pinned
crates, and a runnable two-thread split experiment — and wrote the report with
a plainly-stated recommendation for Roland to accept or amend.

**Headline findings:**
- `Display<State>` is `Send + Sync` *unconditionally* (state is borrowed at
  dispatch, not owned); `DisplayHandle`/`ClientId`/`ObjectId`/`GlobalId` are
  `Send + Sync`. Protocol machinery is freely thread-movable.
- The §7 dispatch-thread / scene-thread split compiles and runs (a real
  `wl_compositor`→`wl_surface`→commit round-trip crosses an mpsc channel; the
  scene owner never touches a Wayland type). No CORE-BOUNDARY §7 conflict.
- Recommendation: consume Smithay's protocol frontend (+ hardware backends
  later), bypass its renderer and `desktop` layers, run `shards = 1` behind a
  `ProtocolHost` seam that keeps shard count an implementation detail. Pin
  `=0.7.0` + lockfile.

## Files changed

| File | Change |
|------|--------|
| `tools/spikes/smithay-threading/Cargo.toml` | New. Standalone spike crate (own `[workspace]`, excluded from `make test`); deps smithay 0.7 + wayland-server/client 0.31. |
| `tools/spikes/smithay-threading/src/main.rs` | New. Q1 static `Send`/`Sync` assertions + Q2 split experiment (dispatch thread + scene thread + one scripted client). Runs clean, exit 0. |
| `tools/spikes/smithay-threading/Cargo.lock` | New (committed on purpose — authoritative version-pin evidence). |
| `tools/spikes/smithay-threading/.gitignore` | New. Ignores the nested `/target`. |
| `docs/smithay_threading_spike.md` | New. The report: version pins, findings per question with evidence, recommendation + tradeoffs, implications for M0 task 3, and the drafted (un-appended) decision-log entry (§8). |
| `docs/parhelion_project_index.md` | Registered the report; updated "Current state" (M0 task 2 done, spike dir noted). |
| `docs/diary.md` | Appended the spike narrative (`#discovery`, `#bug`, `#tradeoff`, `#open-question`). |
| `docs/sessions/_session_2026-07-24_smithay-spike.md` | This summary. |

Also present as pre-existing uncommitted edits (not this session's work):
`CLAUDE.md`, `docs/CORE-BOUNDARY.md`, `docs/VISION.md` (M — from the scaffolding
session), and untracked `docs/prompts/prompt_01_smithay_spike.md`.

## Build / test result

- Spike: `cd tools/spikes/smithay-threading && cargo run` → builds clean, runs
  to `[Q2] RESULT: ... PASS`, **exit 0**.
- Workspace gate: `make test` at repo root → **0 failed** (0 tests, as before),
  clippy `-D warnings` clean. The spike is its own workspace and is not compiled
  by the gate — confirmed.

## Follow-ups for Roland

1. Read `docs/smithay_threading_spike.md`; confirm or amend the recommendation.
2. On confirmation, append §8's drafted entry to
   `docs/parhelion_decision_log.md` and strike the "Pending: Smithay threading
   fit" item (a small, clearly-scoped change — no separate session needed).
3. When the `crates/core` protocol layer lands (M1), migrate the Q1 static
   `Send`/`Sync` assertions in as a real test (regression guard against a future
   Smithay bump quietly removing `Display: Send`).
