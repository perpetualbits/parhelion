# Parhelion — Diary

Dated, informal, append-only. What was attempted, what surprised us, what to
pick up next. The diary is narrative memory; the decision log is settled
reasoning. Entries are tagged (`#core`, `#dialect`, `#invariant`,
`#design-decision`, `#bug`, `#discovery`, `#tradeoff`, `#open-question`, and
per-session tags like `#scaffolding`).

---

## 2026-07-24 — Scaffolding the repository `#scaffolding`

First session. Installed the founding documents and stood up the cargo
workspace. A few things worth recording:

- **`download/` vs `downloads/`.** The landing directory arrived named
  `download/` (singular); `CLAUDE.md`'s canonical layout says `downloads/`.
  Rather than guess, asked Roland — rename to the canonical plural, and
  gitignore it (it is a provenance landing area; the tracked copies live at
  their installed paths). `#design-decision`

- **The name is settled.** Mid-session Roland stated the official name is
  **Parhelion** — no longer a working title. Logged it and resolved the
  Pending item. `VISION.md` §7 still frames the name as a placeholder ("Final
  choice is ADR-0001"); that is stale now, but it is design-doc prose, so it is
  flagged for Roland rather than edited from a scaffolding session — documents
  are authoritative and amended on the design side. `#design-decision`
  `#open-question`

- **Stale cross-references in VISION.md.** VISION.md predates two later
  decisions: it points at a `decisions/` ADR directory and `DIARY.md`, but the
  project settled on a single `docs/parhelion_decision_log.md` and a lowercase
  `docs/diary.md`. Reported, not fixed (installing docs unmodified was the
  instruction). Worth a cleanup pass on VISION.md when Roland next touches it.
  `#discovery`

- **Empty directories.** `tools/` and `tmp/` from the layout were not created —
  git cannot track empty dirs, `tmp/` is gitignored anyway, and neither has
  content. Same rule the milestone plan applies to crates: things appear when
  the work needs them. `docs/plans/` got a `.gitkeep` because it is an active
  part of the session-start workflow (new sessions look for `mN_tasks.md`
  there). `#tradeoff`

- **Lints.** Workspace lints are `warn`, with `-D warnings` applied only in the
  `make test`/CI path so local iteration stays quiet while CI stays strict.
  Edition 2024 on rustc 1.95.0; `resolver = "3"`. The empty skeletons compile,
  clippy is clean, and `make test` runs 0 tests green — the honest M0-task-1
  baseline. The first rig that lands will need to prove it can fail once
  (CLAUDE.md's rule), but there is nothing to fail yet.
