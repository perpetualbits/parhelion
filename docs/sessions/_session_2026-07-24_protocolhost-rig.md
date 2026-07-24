# Session summary — 2026-07-24 — ProtocolHost, protocol rig, M0 closure (M0 task 3b)

**Prompt:** `docs/prompts/prompt_03_protocolhost_rig.md`
**Milestone:** M0 task 3b — the final M0 task.

## What was built

Promoted the Smithay threading spike into production: the real `ProtocolHost`
(Wayland protocol frontend at `shards = 1`), a minimal scene ledger, static
`Send`/`Sync` regression guards, and the protocol test rig — then closed M0.

- **`crates/core/src/ledger.rs`** — `SurfaceId`, `ClientKey` (core-assigned
  tokens), `LedgerMsg`, `Ledger`, `SurfaceRecord`. Pure: zero `wayland-server`
  types. Tracks only live surfaces + a commit flag + owner — nothing M1's scene
  graph will fight. 3 unit tests.
- **`crates/core/src/protocol.rs`** — `ProtocolHost`: spawns the dispatch thread
  (owns `Display<State>`, advertises `wl_compositor` via
  `smithay::wayland::compositor`), runs `calloop` (Display fd as `Generic`,
  control channel for admit/shutdown), and owns the consumer-side `Ledger`.
  `CompositorHandler` translates `new_surface`/`commit`/`destroyed` into
  `LedgerMsg`s carrying only `Send` tokens. Static guards as `const _: fn()`.
- **`crates/core/src/lib.rs`** — crate docs + module wiring.
- **`crates/harness/src/protocol_rig.rs`** — `ScriptedClient`: in-process
  `wayland-client` over a socketpair (connect/create_surface/commit/destroy/
  roundtrip).
- **`crates/harness/tests/protocol.rs`** — the four first tests (below).

## The four ProtocolHost interface requirements — where each lives

1. **Client→shard assignment at accept time:** `ProtocolHost::add_client(UnixStream)`
   (`protocol.rs`) — the single seam both an external `ListeningSocket::accept`
   loop and the rig use; routes the stream to shard 0 via the control channel;
   nothing outside the module names a `Display`.
2. **Dispatch thread owns `Display` + thin `State`; only `Send` tokens cross:**
   `run_dispatch` owns the `Display`; `State` holds no scene data; every
   `LedgerMsg` carries `SurfaceId`/`ClientKey`, never a `WlSurface`.
3. **Receiving side is the minimal `Ledger`:** owned by `ProtocolHost`, folded by
   `sync()`/`wait_until()`; module doc states plainly it is not the scene graph.
4. **Globals advertised identically per shard:** `CompositorState::new` once per
   `Display`; shard-count-agnostic path.

## Test results

`make test` → **green. 20 tests, 0 failed** (was 13; +3 core ledger unit, +4
protocol rig), clippy `--all-targets -D warnings` clean.

Protocol rig tests (`tests/protocol.rs`), all passing:
- `create_and_commit_surface_appears_in_ledger` — surface live + committed.
- `destroy_surface_removes_it_from_ledger` — gone after `wl_surface.destroy`.
- `client_disconnect_cleans_up_surfaces` — gone after client drop (waits on the
  ledger-empty condition, no sleep).
- `two_clients_are_attributed_independently` — two clients on one shard, ledger
  attributes 1 vs 2 surfaces to the right `ClientKey`.

Robustness: ran the protocol suite **20×, 20/20 green** (no concurrency flake).
Guard check: temporarily asserting a non-`Send` type broke the **build** with
"cannot be sent between threads safely" — proving the guards are compiled, not
dead code — then reverted to green.

## M0 acceptance — item-by-item

| Acceptance item | Result |
|-----------------|--------|
| `make test` green locally | ✅ 20 tests, clippy clean. |
| `make test` green in CI | ✅ expected — CI runs the same `make test`; workflow is self-contained (no apt, pure-Rust deps). Not executed in this session (no push). |
| headless golden test passes | ✅ `tests/golden.rs`. |
| deliberately broken golden test fails (rig proven able to fail) | ✅ `tests/prove_can_fail.rs` meta-tests (one-pixel + one-column-shift), themselves green. |
| Smithay decision logged | ✅ decision log "2026-07-24 — Smithay threading fit" (landed this task's step 0 previously). |
| project index lists every document | ✅ verified. |

All green → milestone plan M0 stamped **Status: complete 2026-07-24**.

## Files changed

| File | Change |
|------|--------|
| `crates/core/Cargo.toml` | Added `smithay = "=0.7.0"` (default-features off, `wayland_frontend` only; documented). |
| `crates/core/src/lib.rs` | Crate docs; `pub mod ledger; pub mod protocol;`. |
| `crates/core/src/ledger.rs` | New. Scene ledger + core tokens + unit tests. |
| `crates/core/src/protocol.rs` | New. `ProtocolHost`, dispatch thread, `CompositorHandler`, static guards. |
| `crates/harness/Cargo.toml` | Added `parhelion-core` (path) and `wayland-client = "0.31"`. |
| `crates/harness/src/lib.rs` | `pub mod protocol_rig;`; updated crate doc. |
| `crates/harness/src/protocol_rig.rs` | New. `ScriptedClient`. |
| `crates/harness/tests/protocol.rs` | New. The four protocol-rig tests. |
| `docs/parhelion_milestone_plan.md` | M0 `Status: complete 2026-07-24` line. |
| `docs/parhelion_project_index.md` | Current-state: M0 complete. |
| `docs/diary.md` | `#core`/`#harness` entry. |
| `Cargo.lock` | `smithay` + `wayland-client` trees resolved (committed). |

No new design doc (per subsystem table, `ProtocolHost` is governed by
CORE-BOUNDARY §3/§7 until it earns one).

## Notes for Roland

- Smithay delegate path worked as the decision intended — no raw-`wayland-server`
  fallback and no poll-loop fallback were needed; both preferred options
  (`smithay::wayland::compositor`, calloop) held.
- One `unsafe` block in the codebase: `NoIoDrop::get_mut` in `protocol.rs`
  (SAFETY-commented, scoped `#[allow(unsafe_code)]`) — the standard
  wayland-server-on-calloop pattern.
- Out of scope, as specified: xdg-shell/shm/input/frame-callbacks, the renderer
  seam, scene-graph types, shards > 1, and the scene→client reverse direction
  (M1's first protocol task).
