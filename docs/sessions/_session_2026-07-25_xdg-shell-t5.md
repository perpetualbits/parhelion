# Session summary — 2026-07-25 — xdg-shell minimal + mapping-semantics migration (M1 T5)

**Task:** M1 T5 (prompt 08) — minimal `xdg_wm_base`/`xdg_surface`/`xdg_toplevel`
lifecycle (configure/ack, map/unmap, title/app_id, ping/pong, protocol errors),
C10 fallback placement, and with it the **mapping-semantics migration**: only
mapped toplevels — and core-injected content — are displayed; a roleless
`wl_surface` never composites.

**Build/test result:** `make test` — **69 tests green** (up from T4's 56, +13),
clippy clean, zero warnings.

**Goldens re-blessed: none.** The four T3 shm goldens and all T1 scene goldens are
byte-identical after the migration, because the first toplevel's C10 cascade
placement is the output origin — exactly where the pre-T5 raw-commit path put
content. One golden was **added** (`xdg_cascade`), and its rig was verified to
reject a deliberate one-pixel placement drift (`CASCADE_STEP_Y` 32 → 33 produces
100 differing pixels with `actual`/`golden`/`diff` artifacts), then reverted.

**Seam checks:** `smithay::desktop` and `smithay::backend::renderer` appear
nowhere in the workspace (grep-verified, comments included). Flush ownership
unchanged: exactly one `flush_clients` site.

**T2's callback tests passed unmodified** — the prompt's stop-and-report did not
fire. Firing is not visibility-gated (§8.3), so the attach-less callback proof
keeps its meaning on what is now a definitively invisible surface.

## Files changed

### Created
- `crates/harness/tests/xdg.rs` — 10 tests: the configure/ack dance (exactly one
  configure at 0×0, then map); `roleless_surface_is_never_displayed` (the
  migration's headline, driven from the wire); title/app_id into canonical state;
  ping→pong; three protocol errors, each asserting the **specific code**
  (buffer-before-ack → `xdg_surface.not_constructed` 1; bad ack serial →
  `xdg_wm_base.invalid_surface_state` 4; second *different* role →
  `xdg_wm_base.role` 0); unmap-on-destroy and unmap-on-null-attach with damage
  read off the render counters (the latter also proving the re-map needs a fresh
  configure); cascade determinism.
- `crates/harness/tests/xdg_render.rs` — `xdg_cascade` golden: two clients, two
  toplevels one cascade step apart, later one on top.
- `crates/harness/goldens/xdg_cascade.png` — the new golden.
- `docs/sessions/_session_2026-07-25_xdg-shell-t5.md` — this summary.

### Modified — core
- `crates/core/src/scene/node.rs` — `NodeRole` (`None`/`Toplevel`/`CoreOwned`) +
  `ToplevelRole { title, app_id }`; `SceneNode.role`; the role gate in
  `is_visible`.
- `crates/core/src/scene/state.rs` — `set_role`, `set_title`, `set_app_id`;
  updated `ProtocolEvent` doc (visual state rides closures, not the `Copy` event);
  three unit tests updated/added (roleless invisibility, role-clear unmap,
  metadata).
- `crates/core/src/scene/mod.rs` — export `NodeRole`, `ToplevelRole`.
- `crates/core/src/scene/thread.rs` — `place_solid` assigns `CoreOwned` (the
  scene-injected / C10-fallback door) and says so in its docs.
- `crates/core/src/protocol.rs` — the bulk: `XdgShellState` +
  `delegate_xdg_shell!` + `XdgShellHandler` (`new_toplevel`, `toplevel_destroyed`,
  `title_changed`, `app_id_changed`, `client_pong`, popups dismissed via
  `send_popup_done`); the commit-path lifecycle (ensure-configured gate, mapping
  commit carries the C10 placement, null attach unmaps and re-arms the dance,
  initial configure sent for any unconfigured toplevel); `CASCADE_*` constants;
  `ToplevelEntry` bookkeeping; `Control::PingClients` +
  `ProtocolHost::{ping_clients, pongs_received}`; an **empty** `SeatState` +
  behaviourless `SeatHandler` impl, forced by `delegate_xdg_shell!`'s popup
  dispatch bound (no `wl_seat` global — that is T6).

### Modified — harness
- `crates/harness/Cargo.toml` — `wayland-protocols` (client feature).
- `crates/harness/src/protocol_rig.rs` — xdg client machinery (auto-pong,
  configure-serial queue, toplevel configure sizes); `map_toplevel` /
  `map_toplevel_with` (full dance, custom draw between configure and mapping
  commit); the individual dance steps for conformance tests; positioner/popup for
  the double-role provocation; `RigProtocolError` +
  `protocol_error`/`expect_protocol_error`.
- `crates/harness/tests/shm_render.rs` — all four tests migrated to
  `map_toplevel*` (goldens unchanged).
- `crates/harness/tests/damage.rs` — scene-injected `map_node` gives nodes the
  toplevel role; the CoW isolation test migrated to `map_toplevel`.
- `crates/harness/tests/protocol.rs` — `null_attach_unmaps_the_surface` migrated;
  the three T2 tests untouched.

### Modified — docs
- `docs/scene_graph_v1.md` — new **§10 Mapping semantics and roles (T5)** (the
  rule, the lifecycle, the two Smithay findings, C10 placement constants, what T5
  explicitly did *not* change, the test list); §2 node table + visibility
  sentence; header downstream line; old §10 renumbered to §11.
- `docs/parhelion_decision_log.md` — three entries: the mapping-semantics
  migration; C10 cascade placement; protocol-error assertion capability (with the
  error-code deviation recorded).
- `docs/diary.md` — T5 entry (`#design-decision`, `#core`, `#harness`,
  `#discovery`, `#tradeoff`, `#bug`).
- `docs/parhelion_project_index.md` — current state, document row.
- `project-map.js` — T5 node/parts marked done, files and specs updated.

## Notes for the next session (T6)

- The empty `SeatState` in `protocol.rs` is where `wl_seat` attaches; the
  `SeatHandler` impl already names `WlSurface` as all three focus types.
- Focus policy ("topmost mapped toplevel") now has a real notion of *mapped* to
  build on: role + source, no extra state needed.
- Popups are still dismissed on sight; if T7's terminal demands one, that is the
  standing stop-and-report.
