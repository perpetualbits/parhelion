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
| `docs/smithay_threading_spike.md` | M0 task 2 investigation spike: can Smithay be driven inside CORE-BOUNDARY §7, and which layers we consume. Report + recommendation; decision landed 2026-07-24. | Installed · complete |
| `docs/harness_design.md` | Canonical design for the test harness: frame/golden format, comparator tolerance policy, blessing workflow, determinism contract, failure artifacts (P8). | Installed, authoritative · Draft v0.1 |
| `docs/scene_graph_v1.md` | Canonical design for the scene graph, render loop, and snapshot mechanism (M1 T1–T4): node model (born-3D-ready/2.5D-implemented), texture-source seam incl. real `wl_shm` copy-at-commit (§3.1, T3), §7 thread ownership, snapshot semantics, CPU compositor, the reverse path (§8, T2), **damage tracking v1 — region algebra / retained-frame rendering / partial-copy CoW (§9, T4)**, and **mapping semantics + roles (§10, T5: the role gate, the xdg toplevel lifecycle, C10 cascade placement)**. Absorbs the M0 ledger. | Installed, authoritative · Draft v0.1 |
| `docs/parhelion_project_index.md` | This file. | Living |
| `docs/diary.md` | Running narrative diary; the why behind non-obvious choices, tagged. | Living |
| `docs/sessions/` | One summary per Claude Code session (files changed, build/test result). | Living · `_session_2026-07-24_scaffolding.md` |
| `docs/plans/` | Per-milestone task breakdowns (`mN_tasks.md`), written at each milestone's start. | `m1_tasks.md` (M1 "One window, honestly", T1–T7) |
| `docs/prompts/` | Task prompts authored in the chat project for Claude Code. | `prompt_00_scaffolding.md`, `prompt_04_scene_graph_v1.md`, `prompt_05_frame_callbacks_backpressure.md`, `prompt_06_shm_seam_check.md`, `prompt_08_xdg_shell.md` |
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
| Core: scene graph, render loop, snapshot | `crates/core/` (`scene/`, `render.rs`) | `docs/scene_graph_v1.md` | Scene state + snapshot + CPU compositor v1 (M1 T1) |
| Core: protocol frontend (`ProtocolHost`) | `crates/core/src/protocol.rs` | `docs/CORE-BOUNDARY.md` §3 (C3), §7 | `wl_compositor` + `wl_shm` + `xdg_wm_base`; publishes to the scene (ledger absorbed) |
| Backends (headless, winit, DRM/KMS) | `crates/backend-*/` | (no standalone doc yet) | `backend-headless` renders the M0 test pattern to memory |
| Test harness (golden + protocol rigs) | `crates/harness/` | `docs/harness_design.md` | Golden rig present; protocol rig is task 3b |
| Supervisor (P0) | `crates/supervisor/` | `docs/CORE-BOUNDARY.md` §6, §8 | Not yet (from M4) |
| Reference policy daemon (S1) | `crates/policyd/` | (no standalone doc yet) | Not yet (from M4) |
| Vendored SPINE core spec | `third_party/spine/` | ENO's spec at pinned v0.4 — read-only | Dir present, empty |

Future crates (`backend-winit`, `backend-drm`, `supervisor`, `policyd`) are not
workspace members yet; they appear at the milestone that needs them.

---

## Current state

- **Milestone:** M1 (One window, honestly) — **T5 complete 2026-07-25**
  (`docs/plans/m1_tasks.md`, `docs/scene_graph_v1.md` §10). xdg-shell minimal:
  `xdg_wm_base`/`xdg_surface`/`xdg_toplevel` with the configure/ack dance,
  map/unmap (null attach and role destroy, with structural damage), title/app_id
  into canonical state, ping/pong, and the three protocol errors asserted **by
  code**. With it the **mapping-semantics migration**: a surface without a role is
  never displayed — only mapped toplevels and core-injected C10/harness content
  composite. Placement is a deterministic C10 cascade until the policy daemon (M4).
  The rig learned the whole toplevel dance (`map_toplevel`) and how to observe
  protocol errors. Next is T6 (seat, input, winit backend). `make test`: 69 tests
  green, clippy clean; **no goldens re-blessed** (the cascade origin is the output
  origin, so T3's shm goldens are byte-identical); new golden `xdg_cascade`,
  verified to reject a one-pixel placement drift.
- **M1 T4** complete 2026-07-25
  (`docs/plans/m1_tasks.md`, `docs/scene_graph_v1.md` §9). Damage tracking v1:
  per-surface damage accumulates, flows through the scene into the snapshot as an
  output-space region, and the retained-frame CPU compositor recomputes only
  damaged pixels. Conservative, bounded (coalesces to a bbox past 16 rects),
  subtraction-free region algebra; content-vs-structural damage split; damage-
  aware partial buffer copy with copy-on-write isolation; counters
  (pixels-redrawn / damage-rects / full-damage-frames / bytes-copied). The
  governing property — incremental byte-identical to from-scratch — has its own
  test, verified to fail under sabotage. Next is T5 (xdg-shell). `make test`: 56
  tests green, clippy clean; goldens unchanged.
- **M1 T3** complete 2026-07-24 (`docs/scene_graph_v1.md` §3.1). First real pixels:
  `wl_shm` buffers copied and decoded at commit into a source-neutral
  `PixelBuffer` (`argb8888`/`xrgb8888`), the `wl_buffer` released immediately, and
  the CPU compositor blitting them over solid nodes. **The seam check passed** —
  shm is handled through `smithay::wayland::shm` with no `smithay::backend::renderer`
  (grep-verifiable). Next is T4 (damage tracking). `make test`: 47 tests green,
  clippy clean; shm goldens tolerance-0, discrimination re-demonstrated.
- **M1 T2** complete 2026-07-24 (`docs/scene_graph_v1.md` §8). The reverse path:
  `wl_surface.frame` callbacks fired from the render side over a wait-free
  notice (`FramePresenter`: atomic timestamp + `calloop` ping), the dispatch
  thread the sole owner of every protocol object (§7); flush ownership settled to
  one site; and the backpressure policy (per-client pending-callback cap enforced
  by leaving the socket unread — the I-10 fairness rider). Next is T3 (`wl_shm`
  buffers). `make test`: 40 tests green, clippy clean; the flooding test verified
  to fail without the throttle.
- **M1 T1** complete 2026-07-24 (`docs/scene_graph_v1.md` §1–§7). Scene graph
  v1: canonical scene state on a dedicated scene thread (§7), immutable
  snapshots, a T-render skeleton with a test-controlled tick, and a CPU
  compositor v1 painting the first composited frames. The M0 ledger was absorbed
  into the scene (its tests migrated to scene-state assertions).
- **M0** (Skeleton & harness) — complete 2026-07-24: scaffolding; Smithay
  threading spike (`docs/smithay_threading_spike.md`); headless backend + golden
  rig + CI (`docs/harness_design.md`); `ProtocolHost` shards = 1 + protocol rig +
  static guards.
- **Spikes:** `tools/spikes/smithay-threading/` — reference code for the M0
  task 2 spike; its own cargo workspace, excluded from `make test`.
- **Workspace members:** `parhelion-core` (scene graph, render loop, snapshot,
  and the `ProtocolHost` protocol frontend), `parhelion-harness` (golden +
  protocol + scene-render rigs), `parhelion-backend-headless` (`Frame` +
  `test_pattern` + CPU compositor v1), `parhelion-dialect` (still a skeleton —
  arrives at M3).
