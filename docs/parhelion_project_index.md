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
| `docs/scene_graph_v1.md` | Canonical design for the scene graph, render loop, and snapshot mechanism (M1 T1–T4): node model (born-3D-ready/2.5D-implemented), texture-source seam incl. real `wl_shm` copy-at-commit (§3.1, T3), §7 thread ownership, snapshot semantics, CPU compositor, the reverse path (§8, T2), **damage tracking v1 — region algebra / retained-frame rendering / partial-copy CoW (§9, T4), including the named re-stated-state rule (§9.3)**, **mapping semantics + roles (§10, T5)**, **input, focus, and the nested backend (§11, T6: the InputEvent funnel, the read-mostly focus replica, the recorded §7 winit deviation)**, and **the DRM/KMS backend (§13, M2 T1: T-commit's ownership, connector/mode and the real refresh, the frame handoff, VT semantics, and what is verified how)**. Absorbs the M0 ledger. | Installed, authoritative · Draft v0.1 |
| `docs/parhelion_project_index.md` | This file. | Living |
| `docs/diary.md` | Running narrative diary; the why behind non-obvious choices, tagged. | Living |
| `docs/sessions/` | One summary per Claude Code session (files changed, build/test result). | Living · latest `_session_2026-07-26_drm-session-t1.md` |
| `docs/plans/` | Per-milestone task breakdowns (`mN_tasks.md`), plus operational checklists for work no test can reach. | `m1_tasks.md` (M1 "One window, honestly", T1–T7 — **complete**), `m2_tasks.md` (M2 "On the metal", T0–T8 — T0, T7, T1 complete), `m2_t1_smoke_checklist.md` (the TTY smoke protocol for the DRM backend) |
| `docs/prompts/` | Task prompts authored in the chat project for Claude Code. | `prompt_00_scaffolding.md`, `prompt_04_scene_graph_v1.md`, `prompt_05_frame_callbacks_backpressure.md`, `prompt_06_shm_seam_check.md`, `prompt_08_xdg_shell.md`, `prompt_09_seat_input_winit.md`, `prompt_10_m1_acceptance.md`, `prompt_11_clipboard_m1_close.md`, `prompt_14_drm_session.md` |
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
| Core: scene graph, render loop, snapshot | `crates/core/` (`scene/`, `render.rs`) | `docs/scene_graph_v1.md` | Scene state (roles, damage) + snapshot + CPU compositor v1 |
| Core: protocol frontend (`ProtocolHost`) | `crates/core/src/protocol.rs` | `docs/CORE-BOUNDARY.md` §3 (C3), §7 | `wl_compositor` + `wl_shm` + `xdg_wm_base` + `wl_seat` + `wl_output` + `wl_data_device_manager`; publishes to the scene (ledger absorbed) |
| Core: input funnel and focus routing | `crates/core/src/input.rs` | `docs/scene_graph_v1.md` §11 | `InputEvent` + `FocusMap`; T-input's interface (its thread is M2) |
| Backends (headless, winit, DRM/KMS) | `crates/backend-*/` | `docs/scene_graph_v1.md` §6, §11.4, §13 | `backend-headless` (CPU compositor + `Frame`), `backend-winit` (nested window + `parhelion-dev`), and `backend-drm` (libseat session, atomic KMS, dumb buffers, T-commit) |
| Test harness (golden + protocol rigs) | `crates/harness/` | `docs/harness_design.md` | Golden rig, protocol rig (incl. the toplevel dance, protocol-error assertions, input injection), socket tests |
| Supervisor (P0) | `crates/supervisor/` | `docs/CORE-BOUNDARY.md` §6, §8 | Not yet (from M4) |
| Reference policy daemon (S1) | `crates/policyd/` | (no standalone doc yet) | Not yet (from M4) |
| Vendored SPINE core spec | `third_party/spine/` | ENO's spec at pinned v0.4 — read-only | Dir present, empty |

Future crates (`supervisor`, `policyd`) are not workspace members yet; they appear
at the milestone that needs them. `backend-winit` joined at M1 T6, `backend-drm`
at M2 T1.

---

## Current state

- **M2 T1 — Session, DRM/KMS atomic, dumb buffers, complete 2026-07-26**
  (`docs/plans/m2_tasks.md`; `docs/scene_graph_v1.md` §13). `parhelion-dev --drm`
  boots from a TTY: a libseat session, atomic KMS commits into double-buffered
  dumb buffers, the first connected connector at its preferred mode, and VT
  switches survived (pause drops the in-flight frame; resume re-acquires,
  full-damages the scene, and modesets). **T-commit is born** — its own named
  thread owning the DRM fd, the session, the surface, the buffers, and the vblank
  source — and **its vblank is what ticks T-render**, so the render loop finally
  has a real clock. The headless and nested tick sources are unchanged, which is
  why the whole existing suite still proves what it proved. `wl_output` now
  advertises a refresh computed from the mode's own timings in millihertz, not the
  whole-hertz `vrefresh` field, **retiring T7's 60 Hz claim**. No renderer feature
  from Smithay entered the tree. `make test`: **142 tests green**, clippy clean;
  17 new tests, all of them CI-runnable pure logic.
  **Not verified by any test, by construction:** that a mode-set happens at all,
  that the picture is not sheared, that VT switching returns a correct screen, and
  that the console comes back — those are `docs/plans/m2_t1_smoke_checklist.md`
  and Roland's eyes. **Verdict pending.** Also absent on metal, deliberately:
  **no input and no cursor** until T2.

- **Interactive smoke verified by Roland (2026-07-26):** foot has its title bar;
  a second, independent terminal (`rt`) runs and looks right; `rt` launched from
  inside foot opens a second typeable window with no perceptible lag; resize and
  cursor-over-window both clean. **The two M1 checklist items that no test could
  reach — resize and cursor — are closed.**
- **M2 T7 — Subsurfaces v1, complete 2026-07-26** (pulled to the front of M2; see
  `docs/plans/m2_tasks.md`'s reorder note). The scene grew a tree: parent links,
  parent-relative transforms, sibling order carrying the parent's own slot,
  transitive mapping, sync/desync commit semantics applied as **one atomic scene
  message**, damage and input hit-testing through the tree. The snapshot flattens
  it, so **the renderer did not change** — still a flat back-to-front list. The T0
  conformance test that pinned the silent wrongness **inverted**; `foot` renders
  with its decorations and the acceptance test asserts they composite. `make test`:
  **124 tests green**, clippy clean, five new goldens (discrimination
  re-demonstrated).

- **Milestone: M2 (On the metal) — T0 complete 2026-07-25** (`docs/plans/m2_tasks.md`).
  M1's promissory notes paid or honestly reported. **Paid:** the dispatch-thread
  spin, via a client-intake restructure — one `calloop` source per client (its
  socket `try_clone`d at admission), the aggregate poll fd no longer watched, and
  throttling as a literal source *disable* with hysteresis on re-arm. The spin ends
  by construction: 100 046 loop turns in 300 ms under the old semantics, ~15 now.
  **Reported, not paid:** the subsurface tripwire is impossible — every refusal
  point kills `foot` (it creates nine subsurfaces and puts pixels in eight), so the
  debt stands until **M2 T7** and foot renders undecorated until then. The T7b
  claim that foot never calls `get_subsurface` was a measurement error of mine and
  is corrected in the decision log. `make test`: **110 tests green**, clippy clean.

- **M1 T7b** (2026-07-25) — clipboard v1 and the CI fix. `wl_data_device_manager`
  implemented properly (focus-gating **is** the v1 capability model, satisfying
  I-7's letter; C8/M4 owns the deeper design), with the bytes passing client to
  client through a pipe and never through the core. Drag-and-drop is **refused
  honestly** — `start_drag` cancels the source at once — because grabs meeting the
  focus model is its own design conversation. A real bug fixed: the clipboard's
  owner dying while focus was unchanged left the focused client holding an offer
  backed by a corpse. **Not done, deliberately:** withdrawing the
  `wl_subcompositor` advertisement — it is separable, but doing so makes `foot`
  refuse to start (exit 230), failing M1's own acceptance; the same measurement
  showed foot never calls `get_subsurface`, so the gap is dormant. It stands as a
  stated debt with the advertised global set pinned by test; subsurfaces are
  Roland's call (decision log, Pending). Also fixed the CI failure from the T7
  push: the acceptance test's frame-callback tripwire waited for an unprompted
  second commit, which an idle terminal has no reason to make — it now *causes* the
  redraw it waits for. `make test`: **108 tests green**, clippy clean.
- **Milestone:** M1 (One window, honestly) — **COMPLETE 2026-07-25** (T7;
  `docs/plans/m1_tasks.md`, `docs/scene_graph_v1.md` §12). The acceptance run is
  an automated, headless test (`crates/harness/tests/acceptance.rs`): `foot` — a
  real third-party terminal — connects over a real Wayland socket, maps an xdg
  toplevel, commits shm pixels, keeps drawing because frame callbacks flow,
  receives typed keys through the input funnel, and **redraws 0.62% of the output
  to echo them** (bound 25%), every changed pixel inside the reported damage
  region. Verified to fail when the frame-callback notice is sabotaged. T7 also
  added `wl_output` + `xdg_output`, `wl_data_device_manager` (foot refuses to
  start without a clipboard; implemented properly on Roland's decision, with the
  I-7 capability debt recorded), graceful shutdown for `parhelion-dev` with a
  `--headless` mode, and the conformance sweep. Reported, not fixed:
  `wl_subcompositor` is advertised but the scene ignores subsurfaces. `make test`:
  **101 tests green**, clippy clean. Next: M2 (On the metal).
- **M1 T6** complete 2026-07-25
  (`docs/plans/m1_tasks.md`, `docs/scene_graph_v1.md` §11). Seat, input, and the
  nested backend: `wl_seat` with keyboard + pointer, an xkb keymap, and one
  `InputEvent` funnel every source produces (winit, the rig, and later libinput) —
  applied on the dispatch thread, which still owns every protocol object. Pointer
  routing reads a dispatch-side **read-mostly replica** of the scene's geometry
  rather than querying the scene, so input never waits on rendering (I-2). Focus
  is the C10 fallback (topmost mapped toplevel) until S1 in M4. New crate
  `parhelion-backend-winit`: raw winit + softbuffer (never Smithay's winit
  backend), a resizable window presenting the retained frame, and `parhelion-dev`
  — the M1 interactive artifact, printing a real `WAYLAND_DISPLAY` for external
  clients. **The §7 deviation is recorded, not silent** (§11.2): winit's loop is
  T-input's stand-in; the real thread arrives with libinput in M2. Next is T7
  (the milestone acceptance run: a real terminal). `make test`: 89 tests green,
  clippy clean; no goldens re-blessed. CI gained its first stated system
  dependency, `libxkbcommon` — which had in fact been linked implicitly since the
  protocol layer landed.
- **M1 T5** complete 2026-07-25
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
- **Workspace members:** `parhelion-core` (scene graph, input funnel, render
  loop, snapshot, and the `ProtocolHost` protocol frontend), `parhelion-harness`
  (golden + protocol + scene-render + input + socket rigs),
  `parhelion-backend-headless` (`Frame` + `test_pattern` + CPU compositor v1),
  `parhelion-backend-winit` (nested window + `parhelion-dev`), `parhelion-dialect`
  (still a skeleton — arrives at M3).
