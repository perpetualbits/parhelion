# Parhelion — Decision Log

**Status:** append-only living document.
**Purpose:** record load-bearing decisions with date, source, and
one-line summary. Reasoning stays in subsystem documents; this log
tells you what was decided and where to read more.

New chats: read this immediately after the project index.
(Convention adopted from ENO — `eno_decision_log.md`. The earlier
ADR-style `decisions/0002-*.md` is superseded in *format* by this log;
its content is carried as the 2026-07-24 procedural-content entry
below, reasoning now living in the dialect spec and VISION.md.)

---

## 2026-07-23 — Founding documents

### Compositor is a Wayland-speaking 3D scene-graph engine with microkernel discipline

- **Source:** Parhelion design chat #1.
- **Affects:** `VISION.md` (whole), `CORE-BOUNDARY.md` (whole).
- **Reasoning:** the three goals (Rayland reference S-side,
  non-rectangular windows, 3D desktop objects) are one architecture;
  GNOME's in-process extension model is the anti-pattern.

### Process boundaries follow fault/timing/privilege, not memory

- **Source:** Parhelion design chat #1.
- **Affects:** `CORE-BOUNDARY.md` §4 (placement algorithm), §5 (I-1..I-4).
- **Reasoning:** Rust covers memory safety in-process; IPC buys
  isolation only where blocking, crashing, third-party code, or
  hostile input is possible.

### Two rendering regimes with mandatory collapse

- **Source:** Parhelion design chat #1.
- **Affects:** `VISION.md` Thesis 3, `CORE-BOUNDARY.md` C6, I-9.
- **Reasoning:** damage tracking and game-style full-frame rendering
  solve different problems; the desktop needs both plus a state
  machine that returns to the cheap regime within ≤2 frames.

### Canonical state in core; servers are restartable views

- **Source:** Parhelion design chat #1.
- **Affects:** `CORE-BOUNDARY.md` I-5, §8.
- **Reasoning:** crash-only beats crash-resistant; every control-plane
  protocol carries full resync.

## 2026-07-24 — Procedural content and vocabularies

### Application procedural content travels as shaders + parameters; fixed vocabularies only where compositor-owned

- **Source:** Parhelion design chat #2.
- **Affects:** `parhelion_desktop_dialect.md` §2 reserved
  (`desktop.decor.*`, `desktop.shape.*`), Rayland asset-cache priority.
- **Reasoning:** curated cross-system algorithm libraries for
  *application* content lose to vocabulary lock-in (NeWS, Display
  PostScript precedent); SPIR-V + content-hash cache is the open
  vocabulary. Small closed vocabularies are correct only where the
  compositor owns both ends (shapes, animation IR, decorations).

### Shape is declared, not extracted

- **Source:** Parhelion design chat #2.
- **Affects:** future shape-extension spec; `desktop.shape.*` reserve.
- **Reasoning:** clients know their outline; declared paths give
  analytic occlusion/damage/input regions. Alpha-contour extraction is
  compatibility fallback only.

## 2026-07-24 — Control plane adopts SPINE (full adoption)

### Parhelion's control plane is a SPINE dialect; C7 is a dialect interpreter

- **Source:** Parhelion design chat #3, after review of ENO
  `spine_core_v0_4_design.md`, `nerve_runtime_model.md`,
  `spine_graphics_dialect.md`, `spine_dialect_template.md`.
- **Affects:** `parhelion_desktop_dialect.md` (new, canonical),
  `CORE-BOUNDARY.md` §9 and C7 (to be amended to reference the
  dialect spec), I-6/I-12 (satisfied by construction).
- **Reasoning:** SPINE's seven ops + dialect contract subsume the
  planned bespoke animation IR with stronger structure; opacity rule
  and shape system double as capability boundary and admission
  control.

### Decoupling: vendored pinned core spec, sibling runtimes, deliberate imports

- **Source:** Parhelion design chat #3; requirement stated by Roland
  (ENO must evolve freely).
- **Affects:** `parhelion_desktop_dialect.md` §0.1,
  `third_party/spine/` (to be created).
- **Reasoning:** shared language, separate runtimes. ENO is upstream;
  core changes enter Parhelion only via logged import. NERVE and C7
  share no code in v0.1.

### Event-anchored time, interruption/retraction, submit-time expansion

- **Source:** Parhelion design chat #3.
- **Affects:** `parhelion_desktop_dialect.md` §6, §7.
- **Reasoning:** demos are non-interactive, desktops are; anchors,
  retargetable springs, HOLD-on-retract, and per-property
  last-writer-wins are dialect-level extensions — SPINE core
  untouched (guitar-strum test: gestures with internal time offsets
  are compounds; core ops suffice).

### Signal-shape LNK formalized as typed lock-free channel

- **Source:** Parhelion design chat #3; answers ENO
  `nerve_runtime_model.md` §9.7 from the compositor side.
- **Affects:** `parhelion_desktop_dialect.md` §3; ENO mirror entry
  suggested (Roland's call, in ENO's log, not assumed here).
- **Reasoning:** signal edges cross thread/process boundaries by
  construction in Parhelion, need admission cost accounting and
  defined transport (SPSC latest-value vs queued events) — three
  consumers make it a language-level distinction, not an
  implementation detail.

### Wire form: JSON over the control socket; SPINE binary not adopted (v0.1)

- **Source:** Parhelion design chat #3; "do what is most practical"
  (Roland).
- **Affects:** `parhelion_desktop_dialect.md` §10.
- **Reasoning:** fragments are hundreds of bytes on a local socket;
  64k discipline is ENO's constraint. Revisit on profiling evidence or
  ENO binary-toolchain stability; requires entries in both logs.

## 2026-07-24 — Project name confirmed

### The project's official name is Parhelion (no longer a working title)

- **Source:** Roland, directly, during the M0 scaffolding session.
- **Affects:** `VISION.md` §7 and its re-entrancy header (both still call
  "Parhelion" a placeholder/working name — now stale; flagged for Roland
  to amend, not edited here); the Pending item below (resolved).
- **Reasoning:** the *sundog* metaphor and the Sun Ray → Rayland →
  Parhelion lineage settle it; the alternatives (Sundog, Analemma,
  Firmament, Penumbra) are dropped. The repo, crate names (`parhelion-*`),
  and docs already use it.

## Pending

- Smithay threading fit (investigation task before M0).
- Lock-screen fail-locked design (`CORE-BOUNDARY.md` §6 note).
- Adoption of ENO's project-index + sessions/ + diary structure:
  agreed in principle; instantiate at repo creation.
