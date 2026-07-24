# Prompt 06 — wl_shm buffers, commit semantics, and the seam check

**For:** Claude Code, Parhelion repository.
**Authored in:** the Parhelion chat project, 2026-07-24.
**Milestone:** M1, task T3 (see `docs/plans/m1_tasks.md`).
**Reads first:** `docs/plans/m1_tasks.md` T3; `docs/scene_graph_v1.md`
(texture-source seam section); decision log "Smithay threading fit"
entry 2 (consume/bypass layer table).

---

## Context

First real pixels from a real client. This task implements `wl_shm`
through Smithay's protocol-side machinery and turns the T1 placeholder
into a working `Shm` texture source — and while doing so, performs the
**seam check** flagged since the spike review: Smithay's shm handling
must work without importing its renderer traits. If the seam fights
back, that finding outranks the feature.

## Design constraints

1. **Seam discipline.** Use `smithay::wayland::shm` (state, delegate,
   buffer-contents access) for pool/buffer protocol handling. The
   moment any `smithay::backend::renderer` type is required to reach
   the bytes, stop and report — that is the seam failing, and it is
   design news for Roland, not something to wrap your way around.
   Expected outcome per the spike's layer table: it works cleanly;
   the report states the verdict either way, and the diary records it.
2. **Copy-at-commit, on the dispatch thread.** Buffer contents are
   read where the Wayland objects live: at commit, the dispatch thread
   copies the attached buffer's pixels into an owned, immutable pixel
   block (`Arc`'d), sends it scene-ward as part of the existing
   surface-state message, and releases the `wl_buffer` immediately
   after the copy. Rationale to state in the module doc: correctness
   and client-compatibility first (immediate release lets single-buffer
   clients run); zero-copy and damage-aware partial copies are later
   optimizations (T4 note), and a memcpy on the dispatch thread is not
   the frame path.
3. **Formats v1:** `argb8888` and `xrgb8888` (the two the protocol
   mandates). Composite with deterministic integer src-over for ARGB,
   opaque blit for XRGB. No float in the blend path; goldens stay
   tolerance-0.
4. **Commit semantics per protocol:** attach is double-buffered state,
   applied at commit; attach with nonzero dx/dy — follow the protocol
   version's rule (error on ≥ v5, honor/ignore below with a comment);
   null attach at commit unmaps the content (node loses its source);
   buffer destroyed after our copy is safe by construction — say so in
   a test.
5. **Scene/render side stays source-agnostic.** The renderer blits
   whatever pixel block the snapshot hands it; nothing outside the
   texture-source module knows "shm" exists. The seam sentence from T1
   must remain literally true.

## Task

1. `ShmState` + delegate wiring in `ProtocolHost`; formats advertised.
2. Commit path: attach/commit lifecycle, copy, immediate release,
   message to scene; `TextureSource::Shm` (or your T1 naming) carrying
   the `Arc`'d pixels + dimensions + format.
3. Renderer: blit + integer src-over for snapshot nodes with pixel
   sources, over solids, honoring stacking.
4. Rig + golden tests:
   - Scripted client draws a recognizable pattern (checkerboard with
     an asymmetric marker — orientation matters) into an shm buffer,
     attaches, commits → golden over solid nodes; one XRGB and one
     ARGB-with-transparency variant (blend visible in the golden).
   - Release conformance: client receives `wl_buffer.release` after
     commit; can re-draw and re-commit the same buffer (single-buffer
     client pattern) with the second frame visible in a golden.
   - Null-attach unmap test; destroy-after-commit safety test.
   - Backpressure regression: the T2 flooding test still green with
     commits now carrying real buffers.
5. Docs: `scene_graph_v1.md` texture-source section updated (Shm now
   real; copy/release strategy and its T4 optimization note); seam
   verdict recorded (diary + one line in the T3 decision-log entry if
   the verdict is clean, a full report to Roland if not). Session
   summary; `make test` stated.

## Acceptance

- All prior tests green (40 + new); clippy clean.
- Seam verdict stated explicitly; grep for `smithay::backend::renderer`
  in the workspace finds nothing.
- Goldens tolerance-0 including the ARGB blend.
- Immediate-release behavior proven by the single-buffer re-commit
  test.
- No format/pixel knowledge outside the texture-source module and the
  blit path.

## Out of scope

dmabuf (M2); damage tracking and damage-aware copy (T4); viewporter,
buffer transforms, and scale (post-M1); subsurfaces; any renderer
restructuring beyond the blit.
