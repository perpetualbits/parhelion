# Parhelion — Project Index

> **Re-entrancy header.**
> **Status:** living index · **Kind:** master map of documents and subsystems.
> **What this is:** the first file a new session reads. It lists every document
> with a one-line description and status, states the mandatory reading order,
> and mirrors the subsystem table from `CLAUDE.md`. When a document is added,
> it is registered here in the same session (CLAUDE.md: "Project index").

Parhelion is a Wayland compositor built as a 3D-native scene-graph engine with
microkernel discipline. See `VISION.md` for the why; `CLAUDE.md` (repo root) is
the standing instruction set every session obeys.

---

## Mandatory reading order for a new session

`CLAUDE.md`'s session-start protocol, in order:

1. **This index** (`docs/parhelion_project_index.md`) — the map.
2. **`docs/parhelion_decision_log.md`** — the load-bearing decisions; what was
   decided and where the reasoning lives.
3. **`docs/CORE-BOUNDARY.md` §4 (placement rules) and §5 (invariants
   I-1..I-12)** — the review criteria for every line of code, cited by number.
4. Then the canonical document of whatever subsystem the session is about.

---

## Documents

| Document | Description | Status |
|----------|-------------|--------|
| `CLAUDE.md` (repo root) | Standing instructions for Claude Code sessions: role, session-start protocol, coding rules, repo layout. | Installed, authoritative |
| `docs/VISION.md` | Founding vision and non-negotiables; governs all other docs (P1). | Installed, authoritative · Draft v0.1 |
| `docs/CORE-BOUNDARY.md` | Normative spec: what runs in-core vs. a server, invariants I-1..I-12, threading, failure semantics (P2). | Installed, authoritative · Draft v0.1 |
| `docs/parhelion_desktop_dialect.md` | The `desktop` SPINE dialect: the declarative control plane and C7 interpreter contract. | Installed, authoritative · stub v0.1 |
| `docs/parhelion_milestone_plan.md` | Milestone sequencing M0..M9; each milestone is a usable compositor (P3). | Installed, authoritative · v0.1 |
| `docs/parhelion_decision_log.md` | Append-only log of load-bearing decisions; read second, after this index. | Installed, living |
| `docs/parhelion_project_index.md` | This file. | Living |
| `docs/diary.md` | Running narrative diary; the why behind non-obvious choices, tagged. | Living |
| `docs/sessions/` | One summary per Claude Code session (files changed, build/test result). | Living · `_session_2026-07-24_scaffolding.md` |
| `docs/plans/` | Per-milestone task breakdowns (`mN_tasks.md`), written at each milestone's start. | Empty (placeholder) |
| `docs/prompts/` | Task prompts authored in the chat project for Claude Code. | `prompt_00_scaffolding.md` |
| `docs/archive/` | Superseded documents, kept verbatim; do not edit. | `0002-procedural-content-open-vocabulary.md` (superseded in format by the decision log) |
| `third_party/spine/` | Vendored, pinned SPINE core spec (v0.4) from ENO; read-only. | Empty — pending Roland copying the spec files |

---

## Subsystems — canonical table

Mirrors `CLAUDE.md`'s subsystems table. Each subsystem has exactly one canonical
document; never create a parallel document on the same topic.

| Subsystem | Path | Canonical document | Present now? |
|-----------|------|--------------------|--------------|
| Vision & principles | — | `docs/VISION.md` | Yes |
| Core boundary / process model | `crates/*` (governs all) | `docs/CORE-BOUNDARY.md` | Yes |
| Control plane (`desktop` dialect, C7) | `crates/dialect/` | `docs/parhelion_desktop_dialect.md` | Skeleton crate |
| Milestones | — | `docs/parhelion_milestone_plan.md` | Yes |
| Core (scene, render loop, protocol) | `crates/core/` | `docs/CORE-BOUNDARY.md` §3, §7 (until it earns its own doc) | Skeleton crate |
| Backends (headless, winit, DRM/KMS) | `crates/backend-*/` | (no standalone doc yet) | `backend-headless` skeleton only |
| Test harness (golden + protocol rigs) | `crates/harness/` | (no standalone doc yet; M0 creates it) | Skeleton crate |
| Supervisor (P0) | `crates/supervisor/` | `docs/CORE-BOUNDARY.md` §6, §8 | Not yet (from M4) |
| Reference policy daemon (S1) | `crates/policyd/` | (no standalone doc yet) | Not yet (from M4) |
| Vendored SPINE core spec | `third_party/spine/` | ENO's spec at pinned v0.4 — read-only | Dir present, empty |

Future crates (`backend-winit`, `backend-drm`, `supervisor`, `policyd`) are not
workspace members yet; they appear at the milestone that needs them.

---

## Current state

- **Milestone:** M0 (Skeleton & harness). This session completed M0 task 1
  (scaffolding); the headless backend, golden-test rig, protocol rig, CI, and
  the Smithay threading spike are later M0 tasks, each with its own prompt
  (`docs/parhelion_milestone_plan.md` M0).
- **Workspace members:** `parhelion-core`, `parhelion-dialect`,
  `parhelion-harness`, `parhelion-backend-headless` — all empty skeletons.
