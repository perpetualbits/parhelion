# Session summary — 2026-07-24 — wl_shm buffers, commit semantics, seam check (M1 T3)

**Task:** M1 T3 (prompt 06) — first real pixels from a real client via `wl_shm`,
and the seam check: reach buffer bytes through Smithay's protocol frontend
without importing its renderer.

**Build/test result:** `make test` — **47 tests green** (up from T2's 40, +7:
2 compositor unit + 4 shm end-to-end goldens + 1 null-attach; the flooding
regression updated in place to carry real buffers), clippy clean.

**Seam verdict: CLEAN.** `smithay::wayland::shm` reaches the bytes with no
`smithay::backend::renderer` type; grep for it finds nothing in the workspace.
Builds under `default-features = false, features = ["wayland_frontend"]`. The
spike's layer-table prediction held.

**Design decision confirmed with Roland (up front):** decode shm → source-neutral
RGBA `PixelBuffer` at the copy, folding `xrgb`/`argb` into the node's `opaque`
flag, so the blit path stays format-free; keep the `Shm` variant name.

## Files changed

### Created
- `crates/harness/tests/shm_render.rs` — shm end-to-end goldens (`shm_xrgb`,
  `shm_argb`, `shm_recommit`) + release-after-commit and destroy-after-commit
  assertions, with pixel spot-checks guarding the goldens.
- `crates/harness/goldens/shm_xrgb.png`, `shm_argb.png`, `shm_recommit.png` —
  blessed; golden discrimination re-demonstrated for the shm path (a one-row
  marker change is rejected with `actual`/`golden`/`diff` artifacts).
- `docs/sessions/_session_2026-07-24_shm-seam-check-t3.md` — this summary.

### Modified
- `crates/core/src/scene/node.rs` — `PixelBuffer { width, height, rgba }`;
  `TextureSource::Shm(Arc<PixelBuffer>)` (was a unit placeholder); `TextureSource`
  and `SceneNode` drop `Copy` (keep `Clone`).
- `crates/core/src/scene/snapshot.rs` — `SnapshotNode` drops `Copy`.
- `crates/core/src/scene/state.rs` — snapshot builder clones the source; new
  `set_size` (buffer-defined size, placement untouched) and `clear_source`
  (null-attach unmap).
- `crates/core/src/scene/mod.rs` — export `PixelBuffer`.
- `crates/core/src/protocol.rs` — shm wiring: `ShmState`, `ShmHandler`,
  `BufferHandler`, `delegate_shm!`, `wl_shm` advertised. `commit` now applies the
  double-buffered `BufferAssignment`: copy+decode+immediate-release for
  `NewBuffer`, `clear_source` for null attach. `copy_shm_to_pixels` (little-endian
  `[B,G,R,A]` → `[R,G,B,A]`, stride-stripped, hostile-geometry guarded; one
  documented `unsafe` pool read).
- `crates/backend-headless/src/composite.rs` — `Shm` blit arm + `blit_pixels`
  (per-pixel overwrite/`source_over`, clipped); 2 new unit tests. `match &node.source`.
- `crates/harness/src/protocol_rig.rs` — scripted-client shm support: bind
  `wl_shm`, `create_pool`/`create_buffer`/`attach`/`attach_null`, `ShmPool` (temp
  file backing, rewritable), `ShmFormat` enum (so tests avoid `wayland-client`),
  `buffer_releases()`; `WlBuffer.release` observer.
- `crates/harness/Cargo.toml` — `tempfile` dependency (pool backing file).
- `crates/harness/tests/protocol.rs` — flooding regression carries a real shm
  buffer (re-attached each commit); new `null_attach_unmaps_the_surface` test.
- `docs/scene_graph_v1.md` — §3 rewritten (real `Shm`, `PixelBuffer`) + new §3.1
  (copy-at-commit / immediate release / T4 note); §2 table, §7 tests, §9 T3 row,
  header updated.
- `docs/parhelion_decision_log.md` — 2026-07-24 T3 section: seam verdict (clean)
  + copy-at-commit decision.
- `docs/diary.md` — M1 T3 narrative (seam held, decode-at-copy, immediate
  release, `Copy` removal, little-endian byte order).
- `docs/parhelion_project_index.md` — current state (T3 complete, 47 tests) +
  prompts/doc notes.

## Invariants touched (cited in code)

- **Seam / "Smithay threading fit"** — shm consumed via the frontend; renderer
  layer untouched (verified).
- **§7** — copy where the objects live (dispatch thread); only owned `Send` data
  (`Arc<PixelBuffer>`) crosses to the scene.
- **I-1** — the copy is a dispatch-thread memcpy, off the frame path; the blit
  stays integer/bounded.
- **C9** — first real buffer import (shm); dmabuf/token-buffer attach at the same
  seam later.
- **Trust boundary** — `copy_shm_to_pixels` validates buffer geometry against the
  pool before indexing (rejects rather than panics).

## Acceptance (prompt 06)

- All prior tests green + new (47 total); clippy clean. ✓
- Seam verdict stated; `smithay::backend::renderer` grep finds nothing. ✓
- Goldens tolerance-0 including the ARGB blend. ✓
- Immediate-release proven by the single-buffer re-commit test. ✓
- No format/pixel knowledge outside the copy path + blit. ✓

## Notes / follow-ups (later)

- Damage-aware **partial** shm copy and zero-copy (T4).
- `buffer_delta` (attach dx/dy) ignored in M1 with a comment; needs subsurfaces.
