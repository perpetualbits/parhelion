# Prompt 00 — Repository scaffolding and founding documents

**For:** Claude Code, first session on the Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M0, task 1 (see `downloads/parhelion_milestone_plan.md`).

---

## Context

This is a fresh git repository for **Parhelion**, a Wayland compositor.
Roland has placed the founding documents, authored in the chat project,
into `downloads/`:

- `CLAUDE.md` — your standing instructions (this session installs it;
  every later session starts by obeying it)
- `VISION.md`
- `CORE-BOUNDARY.md`
- `parhelion_desktop_dialect.md`
- `parhelion_decision_log.md`
- `parhelion_milestone_plan.md`
- possibly `0002-procedural-content-open-vocabulary.md` (a superseded
  ADR-format file; see step 4)

Read `downloads/CLAUDE.md` **first**, fully. It defines the layout you
are about to create and the discipline this session must already
follow (session summary, diary, project index). This prompt is
scaffolding only: **no compositor code, no crate implementations
beyond empty skeletons.** Do not redesign anything; where this prompt
and CLAUDE.md conflict, stop and ask Roland.

## Task

1. **Create the directory structure** exactly as CLAUDE.md's "Repo
   layout" specifies, omitting crates that CLAUDE.md marks as arriving
   in later milestones (create only `crates/core/`, `crates/dialect/`,
   `crates/harness/`, `crates/backend-headless/` as workspace members
   now). Add a `.gitignore` appropriate for Rust (target/, tmp/) plus
   `downloads/` handling per step 6.

2. **Install the documents** from `downloads/` into their canonical
   locations: `CLAUDE.md` at repo root; the rest into `docs/` under
   their own names. Do not edit their content, with one exception: if
   any document's internal cross-references use bare filenames, they
   remain valid once co-located in `docs/`; verify, and report (do not
   silently fix) any that break.

3. **Create `docs/parhelion_project_index.md`**: a master index in the
   spirit of ENO's project index — every document with a one-line
   description and its status, the mandatory-reading order for new
   sessions (index → decision log → CORE-BOUNDARY §4–§5), and the
   subsystem table mirroring CLAUDE.md's.

4. **Handle the superseded ADR**, if present in `downloads/`: place it
   at `docs/archive/0002-procedural-content-open-vocabulary.md` with a
   one-line header note pointing to the decision-log entry that
   carries its content ("2026-07-24 — Procedural content and
   vocabularies"). Do not otherwise edit it.

5. **Initialize the cargo workspace**: root `Cargo.toml` with the four
   member crates; each crate is a minimal skeleton (`lib.rs` with a
   `//!` module doc naming its purpose and governing design-doc
   section, no functional code). Workspace-level lints: warnings deny
   in CI profile, clippy configured. Rust edition: latest stable.

6. **Create `downloads/README.md`** (one paragraph): this directory is
   the landing area for chat-authored files; Roland manages it;
   installed files may be deleted from here afterward. Ask Roland
   whether `downloads/` should be committed or gitignored — do not
   decide unilaterally.

7. **Create `third_party/spine/README.md`**: states that this
   directory holds the vendored, pinned SPINE core specification from
   ENO (v0.4), that it is read-only for this repo, and that it is
   currently **empty pending Roland copying the spec files from the
   ENO repository** (list the expected files:
   `spine_core_v0_4_design.md`, `spine_binary_format.md`,
   `spine_dialect_template.md`). Do not fabricate their content.

8. **Create a `Makefile`** with targets: `build` (cargo build
   --workspace), `test` (cargo test --workspace + clippy), `fmt`.
   Comment every target per CLAUDE.md's comment rules.

9. **Close the session per CLAUDE.md discipline**, which applies from
   this very first session: write
   `docs/sessions/_session_<date>_scaffolding.md` (every file created,
   build/test result); append a first `docs/diary.md` entry (tagged
   `#scaffolding`, narrating anything non-obvious you encountered);
   append a decision-log entry **only if** you had to make a
   load-bearing choice this prompt didn't specify (prefer asking over
   deciding); ensure the project index lists everything you created.

10. **Verify:** `make build` and `make test` succeed on the empty
    workspace (empty test suites passing is the expected result —
    state it explicitly). Commit with a clear message. **Do not push.**

## Acceptance

- Tree matches CLAUDE.md's layout (minus future-milestone crates).
- All founding docs installed, indexed, unmodified.
- `make test` green; result stated.
- Session summary + diary entry exist and are indexed.
- No design invented, no code beyond skeletons, nothing pushed.

## Out of scope (explicitly)

Headless backend implementation, golden-test rig, protocol rig, CI
configuration, the Smithay threading spike — those are M0 tasks 2+,
each with its own prompt after this scaffolding lands.
