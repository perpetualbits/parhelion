# Session summary — 2026-07-24 — Repository scaffolding

**Milestone:** M0, task 1. **Prompt:** `docs/prompts/prompt_00_scaffolding.md`.
**Scope:** scaffolding only — no compositor code, empty crate skeletons.

## What was done

### Landing directory
- Renamed `download/` → `downloads/` to match `CLAUDE.md`'s canonical layout
  (Roland's decision this session).

### Documents installed (copied unmodified from `downloads/`)
- `CLAUDE.md` → repo root
- `VISION.md`, `CORE-BOUNDARY.md`, `parhelion_desktop_dialect.md`,
  `parhelion_decision_log.md`, `parhelion_milestone_plan.md` → `docs/`
- `0002-procedural-content-open-vocabulary.md` →
  `docs/archive/` with a one-line supersession header prepended (points to the
  decision-log entry "2026-07-24 — Procedural content and vocabularies"; body
  otherwise verbatim).

### Documents created
- `docs/parhelion_project_index.md` — master index: reading order, document
  list, subsystem table mirroring `CLAUDE.md`.
- `docs/diary.md` — first entry, tagged `#scaffolding`.
- `docs/sessions/_session_2026-07-24_scaffolding.md` — this file.
- `docs/plans/.gitkeep` — placeholder so the (currently empty) workflow dir is
  tracked.

### Decision log appended
- New entry "2026-07-24 — Project name confirmed": the official name is
  **Parhelion** (Roland, mid-session); resolved the corresponding "Pending"
  item. `VISION.md` §7 / its header still call the name a placeholder — flagged
  for Roland to amend, not edited here.

### Cargo workspace (edition 2024, rustc 1.95.0)
- Root `Cargo.toml`: 4 members, `resolver = "3"`, `[workspace.lints]`
  (rust: `unsafe_code`/`missing_docs` = warn; clippy: `all` = warn).
- `crates/core`, `crates/dialect`, `crates/harness`, `crates/backend-headless`
  — each a `Cargo.toml` + `src/lib.rs` with a `//!` module doc naming its
  purpose and governing design-doc section. No functional code.

### Other files
- `.gitignore` — `/target/`, `/tmp/`, `/downloads/`.
- `Makefile` — `build`, `test` (tests + clippy `-D warnings`), `fmt`; every
  target commented.
- `downloads/README.md` (gitignored dir), `third_party/spine/README.md`
  (states read-only, pinned v0.4, empty pending Roland copying the three spec
  files).

## Deliberate omissions / non-load-bearing calls
- `tools/` and `tmp/` from the layout were **not** created: git cannot track
  empty directories, `tmp/` is gitignored scratch, and neither has content yet.
  They appear when first needed (same philosophy as future crates).
- Lint levels are `warn` at workspace level for ergonomic local builds; the
  `-D warnings` denial lives in the `make test`/CI path (the "CI profile").

## Cross-reference verification (reported, not fixed — installed docs are authoritative)
- **VISION.md** refers to a `decisions/` directory, `decisions/0000-conventions.md`,
  and `DIARY.md` (uppercase). The repo uses a single `docs/parhelion_decision_log.md`
  and `docs/diary.md` (lowercase); these references are stale/broken. Consistent
  with the decision log's own note that the ADR-style `decisions/0002-*` format
  was superseded. VISION §7 also cites "ADR-0001", which has no home.
- **CLAUDE.md** (at root): its subsystem table cites bare `CORE-BOUNDARY.md §3,§7`
  / `§6,§8`, which do not resolve from the root (file is at `docs/CORE-BOUNDARY.md`).
- **parhelion_desktop_dialect.md** references `spine_core_v0_4_design.md` and
  `spine_dialect_template.md` — these resolve once the vendored spec is copied
  into `third_party/spine/` (pending). ENO's `nerve_runtime_model.md` etc. are
  upstream, never in this repo.
- All bare cross-references among the co-located `docs/` files resolve fine.

## Build / test result
- `make build`: OK (4 crates compile).
- `make test`: OK — 4 crates, **0 tests passed, 0 failed**, plus 0 doctests;
  `cargo clippy --workspace --all-targets -- -D warnings` clean. Empty test
  suites passing is the expected M0-task-1 result.
- `cargo fmt --all -- --check`: clean.

## Not done (out of scope — later M0 prompts)
Headless backend implementation, golden-test rig, protocol rig, CI config, the
Smithay threading spike.
