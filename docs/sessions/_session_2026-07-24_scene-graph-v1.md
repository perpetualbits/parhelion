# Session summary — 2026-07-24 — Scene graph v1 (M1 T1)

**Task:** M1 T1 (prompt 04) — canonical scene state, the §7 thread skeleton at
shards = 1, immutable snapshots, and a CPU compositor v1 that composites the
first real frames. Absorb the M0 scene ledger.

**Build/test result:** `make test` — **37 tests green** (up from M0's 20),
clippy clean, tolerance-0 goldens. Grep confirms no Smithay renderer/desktop/
space types in `crates/core`.

## Files changed

### Created
- `docs/plans/m1_tasks.md` — installed from `downloads/` (Step 0), indexed.
- `crates/core/src/scene/mod.rs` — scene module root, ownership diagram, re-exports.
- `crates/core/src/scene/node.rs` — `SceneNode`, `Transform` (enum: Identity/Translate; extensible), `TextureSource` (Solid/Shm + the Rayland seam sentence), `SurfaceId`/`ClientKey`.
- `crates/core/src/scene/state.rs` — `Scene` (canonical state), `ProtocolEvent` (succeeds `LedgerMsg`), lifecycle apply + visual setters + `snapshot()`; migrated ledger unit tests + new snapshot/visibility tests.
- `crates/core/src/scene/snapshot.rs` — `Snapshot`/`SnapshotNode` (immutable, owned, back-to-front).
- `crates/core/src/scene/thread.rs` — `SceneThread`/`SceneHandle`/`SceneMsg`: the dedicated scene-owner thread; emit/mutate/place_solid/snapshot/query/wait_until; unit tests.
- `crates/core/src/render.rs` — `Compositor` seam trait, `FrameCounters`, `RenderLoop` (T-render skeleton, test-ticked); unit test with a trivial in-test compositor.
- `crates/backend-headless/src/composite.rs` — `CpuCompositor` (painter's algorithm, integer source-over, edge clipping); 5 unit tests.
- `crates/harness/tests/scene_render.rs` — end-to-end goldens: two-overlap + restacked, clipped, snapshot-isolation; counter assertions.
- `crates/harness/goldens/scene_two_overlap.png`, `scene_two_overlap_restacked.png`, `scene_clipped.png`, `scene_snapshot_isolation.png` — blessed; deliberate-failure demonstrated once (1-px shift → mismatch + artifacts).
- `docs/scene_graph_v1.md` — new canonical subsystem doc.

### Deleted
- `crates/core/src/ledger.rs` — absorbed into the scene graph.

### Modified
- `crates/core/src/lib.rs` — module list (`scene`, `render`; dropped `ledger`) + updated crate docs.
- `crates/core/src/protocol.rs` — `ProtocolHost::new(SceneHandle)`; dispatch thread publishes `ProtocolEvent` via `SceneHandle::emit`; removed `sync`/`ledger`/`wait_until`/`Default`; static guard now covers `ProtocolEvent`/`SceneHandle`.
- `crates/backend-headless/src/lib.rs` — `pub mod composite;`.
- `crates/backend-headless/Cargo.toml` — dependency on `parhelion-core` (backend → core).
- `crates/harness/tests/protocol.rs` — migrated from `host.ledger()`/`host.sync()` to scene-state queries via a `SceneThread`.
- `crates/harness/src/protocol_rig.rs` — module doc example updated to the new scene API.
- `CLAUDE.md` — subsystem table: split the Core row into scene-graph (→ `scene_graph_v1.md`) and protocol-frontend rows.
- `docs/parhelion_project_index.md` — documents table, subsystems table, current state (M1 T1), prompts row.
- `docs/parhelion_decision_log.md` — five M1-T1 entries (3D-ready/2.5D transform; texture-source seam; `Compositor` seam + backend→core dep; scene thread + test-ticked render; ledger absorbed).

## Invariants touched
- **I-5 / C4** — scene state is the canonical state, owned by one thread.
- **§7** — single-owner scene thread; snapshots cross to T-render as owned immutable values; proto→scene edge one-way/async.
- **I-1 / I-3** — no lock shared between the frame path and the scene thread (owned snapshot); publish edge never blocks on a reply.
- **I-12 / C9** — the texture-source seam (Rayland).
- Decision "Smithay threading fit" — no Smithay renderer types in `crates/core` (grep-verified).

## Notes / deliberate M1 simplifications
- T-render is a test-ticked skeleton; the vblank-tied frame scheduler is M2. Documented in `scene_graph_v1.md` §4 and the decision log.
- Snapshot v1 is a full `Vec` copy; §10.3 (persistent sharing) stays open.
- `Shm` texture source is a declared placeholder (rejected by the compositor); T3 implements it.
