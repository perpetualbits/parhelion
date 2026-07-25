# Parhelion — Diary

Dated, informal, append-only. What was attempted, what surprised us, what to
pick up next. The diary is narrative memory; the decision log is settled
reasoning. Entries are tagged (`#core`, `#dialect`, `#invariant`,
`#design-decision`, `#bug`, `#discovery`, `#tradeoff`, `#open-question`, and
per-session tags like `#scaffolding`).

---

## 2026-07-24 — Scaffolding the repository `#scaffolding`

First session. Installed the founding documents and stood up the cargo
workspace. A few things worth recording:

- **`download/` vs `downloads/`.** The landing directory arrived named
  `download/` (singular); `CLAUDE.md`'s canonical layout says `downloads/`.
  Rather than guess, asked Roland — rename to the canonical plural, and
  gitignore it (it is a provenance landing area; the tracked copies live at
  their installed paths). `#design-decision`

- **The name is settled.** Mid-session Roland stated the official name is
  **Parhelion** — no longer a working title. Logged it and resolved the
  Pending item. `VISION.md` §7 still frames the name as a placeholder ("Final
  choice is ADR-0001"); that is stale now, but it is design-doc prose, so it is
  flagged for Roland rather than edited from a scaffolding session — documents
  are authoritative and amended on the design side. `#design-decision`
  `#open-question`

- **Stale cross-references in VISION.md.** VISION.md predates two later
  decisions: it points at a `decisions/` ADR directory and `DIARY.md`, but the
  project settled on a single `docs/parhelion_decision_log.md` and a lowercase
  `docs/diary.md`. Reported, not fixed (installing docs unmodified was the
  instruction). Worth a cleanup pass on VISION.md when Roland next touches it.
  `#discovery`

- **Empty directories.** `tools/` and `tmp/` from the layout were not created —
  git cannot track empty dirs, `tmp/` is gitignored anyway, and neither has
  content. Same rule the milestone plan applies to crates: things appear when
  the work needs them. `docs/plans/` got a `.gitkeep` because it is an active
  part of the session-start workflow (new sessions look for `mN_tasks.md`
  there). `#tradeoff`

- **Lints.** Workspace lints are `warn`, with `-D warnings` applied only in the
  `make test`/CI path so local iteration stays quiet while CI stays strict.
  Edition 2024 on rustc 1.95.0; `resolver = "3"`. The empty skeletons compile,
  clippy is clean, and `make test` runs 0 tests green — the honest M0-task-1
  baseline. The first rig that lands will need to prove it can fail once
  (CLAUDE.md's rule), but there is nothing to fail yet.

---

## 2026-07-24 — Smithay threading spike `#spike`

M0 task 2: an investigation, not production code. Question: can Smithay be
driven inside CORE-BOUNDARY §7's one-owner-per-thread model, and at what layer
do we consume it? Built a standalone spike crate
(`tools/spikes/smithay-threading/`) against real pinned deps and let the
compiler and a running experiment answer.

- **The big surprise.** `Display<State>` is `Send + Sync` *unconditionally* —
  even with a `State` holding a raw pointer it stayed `Send`. The state is
  never stored inside the `Display`; it is only borrowed at
  `dispatch_clients(&mut State)`. So the received wisdom ("Smithay is
  single-threaded, calloop-bound") is about the *idiom*, not the *types*. The
  protocol machinery is freely thread-movable, and nothing forces protocol
  state and scene state onto the same thread. That reframes the whole question
  from "can we?" to "how many shards?". `#discovery` `#invariant`

- **Zero `unsafe impl Send/Sync` in the pure-Rust backend.** The thread-safety
  is honest auto-derivation off an `Arc<Mutex<dyn ErasedState + Send>>` handle,
  not a `unsafe impl` papering over interior mutability. That made me trust the
  compiler's "yes" more than I expected to. `#discovery`

- **The split experiment worked first try after two signature fixes.**
  `insert_client`/`create_global` live on `DisplayHandle`, not `Display`
  (misread the impl blocks initially), and `DisconnectReason` is under
  `backend::`. Once those compiled, a dispatch thread owning the `Display` and
  a scene thread owning a toy scene, talking only over an mpsc channel of
  `ObjectId`s, ran a real `wl_compositor`→`wl_surface`→commit round-trip
  cleanly. The scene function has *no Wayland type in scope* — the isolation is
  structural. That is the §7 T-proto[n]→scene edge, demonstrated. `#core`

- **A shutdown race, briefly.** First run panicked with `BrokenPipe`: the
  dispatch thread tore down the `Display` the instant it saw the commit, so the
  client's second round-trip hit a closed socket. Fixed with a ~50ms grace
  drain before teardown and dropping the redundant round-trip. A spike-only
  artifact (production uses calloop, not a sleep-poll loop), but worth the note:
  crash-only teardown ordering is going to be a recurring theme. `#bug`

- **Churn lives where we don't depend.** Smithay's 2025 release burst
  (0.4→0.7 in five months) broke mostly renderer and DRM APIs — exactly the
  layers the recommendation says to bypass or wrap. The frontend we lean on
  hardest is its calmest layer. Pin exact + lockfile, mirror the SPINE
  vendored-pinned discipline. `#tradeoff`

- **Recommendation** (verdict is Roland's): consume the protocol frontend now
  and hardware backends later, bypass the renderer and `desktop` layers, run
  `shards = 1` behind a `ProtocolHost` seam so shard count is an implementation
  detail. No §7 conflict. Decision-log entry drafted at the end of the report,
  not appended — Roland confirms, then it lands. `#open-question`

---

## 2026-07-24 — Headless backend, golden rig, CI `#harness`

M0 task 3a: the velocity multiplier. A deterministic headless frame producer, a
golden comparator that provably fails on a wrong frame, and CI. First: landed
the confirmed Smithay spike decision into the log and struck its Pending item.

- **Determinism designed in, not hoped for.** `Frame` is integer-only RGBA8,
  tightly packed, no stride. `test_pattern` uses only integer division for its
  gradients — no float rounding to diverge across machines. Verified three ways:
  two renders are byte-identical (unit test), the PNG encoder is byte-identical
  across calls (unit test), and — the acceptance check — deleting the golden and
  re-blessing regenerated a byte-identical file (`sha256` matched). That last
  one is the guarantee that actually matters for a committed golden. `#discovery`

- **The pattern is adversarial on purpose.** Gradients for tolerance territory,
  a 1px grid + hard patch edge for off-by-one/stride, four distinct corner
  markers for orientation/transpose flips, and an exact `#804020` reference
  patch that must survive PNG round-trip bit-for-bit. The prove-it-can-fail
  meta-test then shifts the whole image one column and confirms the rig screams.
  `#harness`

- **Tolerance policy: 0 by default, loosened per-test only.** CPU frames match
  exactly today; the tolerance/budget machinery is dormant, built for the GPU
  future where drivers rasterise differently. Wrote the rule down in
  `harness_design.md` so nobody reaches for a global fudge factor — that would
  hide regressions in every test at once. `#design-decision`

- **CI needs no apt.** The spike already established the protocol layer needs no
  libwayland; this task uses no Smithay at all and only the pure-Rust `png`
  codec. So the workflow is checkout → toolchain → cache → `make test`, with a
  comment explaining why there is deliberately no system-package step. The
  hardware backends (M2) will earn their own job with real packages. `#tradeoff`

- **Held the line on scope.** No renderer trait, no scene-graph type, no seam
  anticipating M1 — the producer's whole interface is "give me frame N of size
  W×H." M1 designs its seam when the scene graph is on the table. It was
  tempting to add a `Renderer` trait "for later"; resisted, per CLAUDE.md. The
  `frame` argument is the one concession to the future, and it does real work
  (moves a cursor bar) rather than being speculative. `#open-question`

- `make test`: 13 tests pass, clippy `-D warnings` clean. png 0.17.16 pinned via
  the lockfile (0.18 exists; 0.17 is the minimal known-good).

---

## 2026-07-24 — ProtocolHost, protocol rig, M0 closure `#core` `#harness`

M0 task 3b: promote the spike into `crates/core`. The real `ProtocolHost`
(shards = 1), a minimal scene ledger, the protocol rig, static regression
guards. This closes M0.

- **Smithay's `CompositorHandler` maps 1:1 onto the ledger.** `new_surface` /
  `commit` / `destroyed` are exactly SurfaceCreated / SurfaceCommitted /
  SurfaceDestroyed. So the delegate path the frontend decision points at needed
  no raw-`wayland-server` fallback — `delegate_compositor!(State)` supplies every
  Dispatch impl, and my `State` is just three handler methods plus id
  bookkeeping. Minimal feature set: `smithay` with `default-features = false,
  features = ["wayland_frontend"]` builds without any of the DRM/renderer stack.
  `#discovery`

- **Core-assigned tokens for both surfaces AND clients.** The spike celebrated
  "no Wayland type in scope" on the scene side; I kept that honest by assigning
  a `ClientKey` at admit time (stored in the client's `ClientData`) as well as a
  `SurfaceId` at create. The ledger module imports zero `wayland-server` types —
  it is pure core tokens. `new_surface` reaches the owning client's key via
  `surface.client().get_data::<ClientState>()`, and `disconnected` has it on
  `&self`. That symmetry made the two-cleanup-paths problem (explicit destroy vs
  client disconnect) fall out cleanly. `#design-decision`

- **calloop owns the Display.** `Generic::new(display, READ, Level)` hands the
  Display back as the callback's 2nd arg; a `calloop::channel` carries
  admit-client/shutdown into the thread and wakes it. One `unsafe`:
  `NoIoDrop::get_mut` to reach the Display calloop guards — SAFETY-commented
  (we only dispatch/flush, never drop the fd) and scoped `#[allow(unsafe_code)]`
  since the workspace lint warns on unsafe. This is the production substrate, so
  no poll-loop fallback was needed. `#core`

- **Determinism without sleeps.** The create/commit/destroy tests synchronise on
  the client's `roundtrip` (returns only after the server processed the
  requests, and protocol ordering guarantees the ledger messages are already
  enqueued — so a non-blocking drain sees them). The disconnect test has no
  round-trip to lean on, so it waits on a *condition* (`wait_until` the ledger
  empties) rather than a fixed delay. Ran the protocol suite 20× — 20/20 green,
  no flakiness. `#harness`

- **Static guards break the BUILD, not just tests.** The Q1 Send/Sync facts live
  in library code as `const _: fn() = || { assert_send::<Display<State>>(); … }`
  — type-checked by `cargo build`. Verified by temporarily asserting `*mut u8`:
  the build failed with "cannot be sent between threads safely," exactly the
  regression signal a future Smithay bump that dropped `Display: Send` would
  trip. Reverted; clean. `#invariant`

- **M0 closed.** Walked the acceptance list item by item (all green) and stamped
  the plan's M0 section `Status: complete 2026-07-24`. `make test`: 20 tests,
  clippy `-D warnings` clean.

---

## 2026-07-24 — Scene graph v1 (M1 T1) `#core` `#scene`

- **The narrow line, in the type system.** The hard part of T1 was not code, it
  was Thesis 1 vs Thesis 3: born 3D-ready, implemented 2.5D. Landed on
  `enum Transform { Identity, Translate{dx,dy} }` — the compositor's `match` is
  exhaustive over what exists, so *no transform math beyond translation is
  reachable*, yet adding a real affine/4×4 is a new variant, not a redesign. An
  enum is 3D-ready in the vocabulary without being a half-built matrix path. The
  alternative (a struct with reserved rotation/scale fields) would have carried
  dead fields and invited a half-working code path; rejected. `#design-decision`
  `#tradeoff`

- **Keeping `Frame` out of the core.** The render loop is C5 (core), but the
  `Frame` it paints and the CPU compositor are backend concerns — and the M0
  test-pattern code lives in `backend-headless`. If the core called the
  compositor directly it would depend on a backend crate; if the compositor held
  the core's `Snapshot` and the core also drove it, that is a dependency cycle.
  Resolved with one core-defined `Compositor` trait: core drives it via
  `RenderLoop::tick` and never names `Frame`; `backend-headless` implements it
  and depends on `core` (one direction). This is the exact C5↔C1 seam M2's DRM
  backend plugs into — a seam the task needs, not a speculative abstraction.
  `#design-decision` `#core`

- **Honest about the render thread.** §7 wants T-render to own the GPU context
  and consume snapshots on the frame path. There is no GPU, no vblank, no frame
  deadline yet — so a "real" render thread spinning a loop would be theatre.
  Made T-scene a real dedicated thread (the load-bearing single-owner property)
  but left T-render a **skeleton driven by a test-controlled `tick()`**. This is
  the honest RT stance from VISION: don't pretend a clock exists before the
  hardware does. Documented as a deliberate scaffold in `scene_graph_v1.md` §4
  and the decision log, not buried. `#tradeoff` `#open-question`

- **Single FIFO gives free determinism.** Both the protocol dispatch thread and
  the test thread send into the scene's *one* mpsc inbox. After
  `client.roundtrip()` the dispatch thread has already emitted (during
  `dispatch_clients`, before the sync reply unblocks the round-trip), so a
  later `scene.query(...)` from the test observes those events — happens-before +
  FIFO, no sleep. The disconnect case (no round-trip) waits on a condition via
  `SceneHandle::wait_until`, parked on the scene thread where the `ClientGone`
  arrives. `#harness`

- **Two self-inflicted bugs, both caught fast.** (1) In the ProtocolHost rewire I
  "cleaned up" the shutdown flag into a `let mut stop` captured by the `move`
  control closure — a bool is `Copy`, so the closure got its own copy and the
  loop would never see `stop`, hanging on drop. Caught it re-reading before
  running; restored the `State.stop` field the M0 code used for exactly this
  reason. (2) The prove-it-can-fail discipline earned its keep: perturbing a
  node's x by one pixel made `scene_two_overlap` fail with `actual`/`golden`/
  `diff` artifacts, and reversing the compositor's draw order flipped the
  overlap colour — both confirmed the goldens actually discriminate before I
  trusted them. `#bug` `#discovery`

- **Ledger died as designed.** M0's ledger was always a stand-in "the M1 scene
  graph must not fight." Deleted `ledger.rs`; its lifecycle is now
  `Scene::apply(ProtocolEvent)` and its four rig tests assert on scene state.
  `make test`: 37 tests green, clippy clean. `#core`

## M1 T2 — the reverse direction (frame callbacks, flush, backpressure)

- **The reverse edge is deliberately thin.** T-render tells T-proto exactly one
  thing: "a frame was presented at `t`" — an atomic store plus a `calloop` ping,
  wait-free (I-1). Every Wayland object stays on the dispatch thread; the render
  side never posts an event. That single constraint (§7, "one thread touches
  protocol objects") made the whole design fall out: the ping wakes the dispatch
  loop, which drains each surface's committed `frame_callbacks` and sends
  `done(t)`. No cross-thread object posting, no re-audit needed when we shard.
  `#core` `#protocol`

- **Callbacks are NOT gated on visibility, and that's correct.** My first instinct
  was "fire callbacks for surfaces in the snapshot." The attach-less-commit proof
  test kills that: an unmapped surface with a frame request must still get its
  `done`. So v1 fires *all* committed callbacks per tick; occlusion gating is M2
  (needs T4's visibility). Bonus: not consulting the snapshot means snapshot
  semantics are untouched (which the prompt put out of scope). `#protocol`
  `#design-decision`

- **Flush ownership: one site, and it had to move.** M0's flush lived inside the
  `Display` source callback. But `present` (a *different* source) also needs its
  `done` events flushed. Two flush sites would violate "exactly one." Fix: flush
  once in the loop body after `event_loop.dispatch()` returns — every source only
  *enqueues*, one flush pushes it all. `DisplayHandle::flush_clients` exists, so
  the site doesn't need the `Display` back from the calloop source. Grep proves
  it: one `flush_clients`, zero `DisplayHandle` outside `protocol.rs`. `#protocol`

- **The backpressure discovery (`#discovery`).** I expected to bound a flooding
  client to a tight per-event cap. Reading the `rs` wayland-backend killed that:
  `dispatch_single_client` reads a ready socket *to `WouldBlock` in one call* —
  no per-request limit. So the real bound is `cap + one socket-read burst`; the
  throttle stops the *next* read, and the client's own writes block when its
  kernel buffer fills (the true end-to-end backpressure). This also reshaped the
  *test*: to make throttling observable and deterministic I flood A one
  frame+commit+roundtrip at a time up to the bound (no blocking), then pile
  unread extras on and prove they stay unadmitted while B is served in one tick.
  `#discovery` `#tradeoff`

- **The right throttle signal was the non-obvious one.** Bounding the scene
  channel is untestable here — the scene thread drains it so fast the throttle
  never engages, so "an invariant without a test is a wish." The queue a client
  can *actually* grow unbounded is its pending frame-callback backlog, because
  callbacks drain only on a render tick it doesn't control. Throttling on *that*
  is what makes the flooding test fail when the bound is removed (verified:
  backlog jumps to `bound + 2000`). Unscheduling the socket bounds the scene
  direction transitively. `#protocol` `#discovery`

- **v1 cost, taken with eyes open.** A throttled client with unread data keeps the
  level-triggered `Display` source ready, so the dispatch thread spins to keep
  serving others during an active flood. It's the dispatch thread, not the frame
  path (I-1 untouched). Roland's call: keep it simple, note it; the per-client
  readiness fix is M2. `make test`: 40 tests green (+3 T2 rig tests), clippy
  clean. `#tradeoff` `#core`

## M1 T3 — first real pixels (wl_shm) and the seam check

- **The seam held.** The whole point of T3 was the check flagged since the spike:
  can we handle `wl_shm` through Smithay's *protocol* frontend without dragging in
  its renderer? Verdict: clean. `smithay::wayland::shm::with_buffer_contents`
  hands over a raw pointer + `BufferData{width,height,stride,format}` and never
  mentions `smithay::backend::renderer`. It builds under our
  `wayland_frontend`-only feature set (the shm module and the `backend::allocator`
  format helpers it leans on aren't renderer-gated). Grep for the renderer path
  finds nothing in the workspace — even my own doc comments had to be reworded off
  the literal token so the acceptance grep stays honest. `#discovery` `#core`

- **Decode at the copy, not the blit.** I folded the format distinction into the
  copy path: `xrgb8888` → RGBA with alpha forced to 255 + node marked opaque;
  `argb8888` → straight RGBA + not-opaque. Then the compositor's *existing*
  `opaque`/`source_over` machinery just works on a source-neutral `PixelBuffer`,
  and the renderer never learns "shm" exists. Format knowledge lives in exactly
  one place. Roland OK'd this deviation from "carry the format to the blit" up
  front. `#design-decision`

- **Copy-at-commit + immediate release is the load-bearing choice.** Copying the
  bytes into an owned `Arc<PixelBuffer>` at commit and releasing the `wl_buffer`
  right away makes two hard things trivial: single-buffer clients run (they get
  `release` and can redraw at once — the `shm_recommit` golden proves it), and
  destroy-after-commit is safe *by construction* (the scene's copy outlives the
  buffer — a test asserts it). The copy is a memcpy on the dispatch thread, which
  is not the frame path, so I-1 is untouched. Zero-copy is a T4 problem. `#core`

- **`Copy` had to go.** `TextureSource` now holds an `Arc`, so `TextureSource`,
  `SnapshotNode`, and `SceneNode` dropped `Copy` (kept `Clone`). Nice side effect:
  a full-copy snapshot now shares pixel data by ref-count instead of deep-copying
  — snapshots stayed cheap even carrying real buffers. The one code change that
  bit was `n.source.expect(...)` in the snapshot builder (can't move out of a
  borrow) → `n.source.clone().expect(...)`. `#core`

- **shm little-endian byte order is the usual trap.** `argb8888`/`xrgb8888` are
  `0xAARRGGBB` little-endian, so in memory each pixel is `[B, G, R, A]`; the copy
  reorders to the `[R, G, B, A]` the `Frame` wants. Got it right first try by
  writing the exact expected blend value into the test (`[60,120,150,255]` for
  50%-alpha light-blue over grey) and watching it pass before blessing the golden
  — pixel asserts guard the goldens against blessing garbage. `make test`: 47
  tests green (+7), clippy clean; the shm golden re-demonstrated it can fail (a
  one-row marker change is rejected). `#harness` `#discovery`

## M1 T4 — damage tracking (the compositor learns to do less)

- **One property carries the whole task: incremental == from-scratch.** Everything
  else — the region algebra, the retained frame, the partial copy — is plumbing
  around "damage may change cost, never output." I wrote that as a single test
  (`assert_equiv`) that, at every step of an awkward sequence, composites
  incrementally into a retained frame AND from scratch into a blank one, and
  demands byte-identity. It earned its keep immediately (below). `#core`

- **The equivalence test caught two bugs — one mine-in-code, one mine-in-test.**
  The test-bug was instructive: step 7 swapped a node's whole source but I passed
  `ContentDamage::Rects(vec![])` — empty rects = *no* damage, so incremental left
  the stale content. The fix taught the real invariant: the protocol NEVER sends
  empty `Rects` (`build_pixel_block` sends `Full` when there's no usable client
  damage), so `attach_content` can honour exactly what it's given. Under-reported
  damage is the client's bug; the compositor is conservative only where it owns
  the decision. `#core` `#discovery`

- **Content-vs-structural is the split that matters.** A small commit must redraw
  a small region; a move/restack must repaint where the node left and landed. So
  `attach_content` checks whether the extent changed: same extent → honour the
  client's damage rects (proportional); changed → old ∪ new extent (structural).
  Setters (`set_z`, `set_geometry`, …) damage as they mutate. Getting this split
  right is the whole difference between "damage tracking" and "always full". The
  proportionality test proves it with counters: a 10×10 update on a 100×100
  surface redraws ~100 px, not 10 000. `#core`

- **Conservative + bounded + NO subtraction.** The region is a rect list that only
  ever rounds outward; past 16 rects it collapses to its bounding box. I resisted
  subtraction entirely — it's where region code grows teeth (rect-splitting,
  fragment explosions) and nothing in M1 needs it. Over-approximation is always
  safe (repaint a bit more); under-approximation is a bug class (stale pixels).
  Kept our own `Rect`/`Region` rather than pulling a crate or Smithay's region
  types, so the snapshot and backend stay Smithay-free. `#design-decision`

- **Partial copy is really about isolation, not speed.** At commit I patch only
  the damaged buffer region into the surface's block — but via `Arc::make_mut`,
  which clones first *iff* the block is still shared (an in-flight snapshot holds
  it). So the win I care about is that a snapshot's pixels are never mutated under
  it (CoW), byte-checked by a dedicated test driving the real protocol path. The
  byte-count saving (copy only the dirty rect from the mmap) is a bonus. Because
  the scene always holds a ref, make_mut clones every commit — correct, and cheap
  enough for M1; true in-place is a later optimization. `#core`

- **Sabotage confirms the net has holes where it should.** Dropped `set_z`'s
  damage call → the equivalence test failed precisely at the restack step, then
  reverted. A green equivalence test that never fails proves nothing. `make test`:
  56 tests green (+9), clippy clean. `#core` `#harness`

## M1 T5 — xdg-shell, and the day surfaces stopped being windows

- **The migration was the task; xdg-shell was the excuse.** Until today the scene
  composited anything that had committed pixels. That was convenient for four
  tasks' worth of tests and it was simply wrong: Wayland says a surface without a
  role is never displayed. T5 makes the role gate visibility, and the honest way
  to read the change is that "in the scene" now means something narrower than it
  did yesterday — which is why it got a decision-log entry rather than a bullet in
  a feature list. `#design-decision` `#core`

- **"Mapped" wanted to be a bool and shouldn't be.** My first instinct was a
  `mapped` flag on the node, set by the protocol path. Then: mapped means "has a
  role and has committed content", and both of those are already stored. A flag
  would be a second source of truth able to drift from the first — and the drift
  would show up as a window that is invisible-but-mapped, the worst kind of bug to
  chase. So the definition *is* the check. `#core`

- **`CoreOwned` is not a test hatch.** The awkward member of `NodeRole` is the one
  for content the core places itself. It looks like an escape valve for the
  harness's `place_solid`, and I nearly named it that way — but C10 exists
  precisely for the case where every server is dead, and a "server crashed"
  surface has no client to grant it a role. The harness riding the same door is a
  consequence, not the purpose. `#design-decision`

- **Choosing cascade origin (0, 0) cost nothing and saved every T3 golden.** A
  realistic first-window offset (32, 32) would have moved the shm content in four
  goldens and forced a re-bless — re-blessing that would have been legitimate but
  unnecessary. Placing the first toplevel at the origin means the migrated tests
  produce byte-identical frames, so the goldens keep testing what they always
  tested, and the only new golden is the one that exists to show the cascade.
  Re-blessed nothing; that felt like the right kind of boring. `#harness`

- **Two Smithay findings, neither a stop-and-report.** (1) Its toplevel commit
  hook detects unmap through surface state only its *renderer helpers* populate —
  and we use none of them, by design — so that bookkeeping is silently inert for
  us and the core re-arms the initial-configure dance itself on unmap. (2) The
  buffer-before-ack error comes out as `xdg_surface.not_constructed` where the
  spec has a dedicated `unconfigured_buffer`, and the `xdg_surface` object never
  reaches us, so we can't post our own. Both are consequences of the
  frontend/renderer split we deliberately bought; the test pins the code we
  actually send, because pinning the code you *wish* you sent is how a conformance
  test becomes a lie. `#discovery` `#tradeoff`

- **The double-role test almost tested nothing.** I first provoked it with two
  `get_toplevel` calls on one `xdg_surface` — and it passed clean, because
  Smithay's role bookkeeping treats re-assigning the *same* role as idempotent. A
  test that green-lights the violation it is named after is worse than no test. The
  real provocation is a *different* role: one `wl_surface`, two `xdg_surface`s,
  toplevel on one and popup on the other → `xdg_wm_base.error.role`. That is also
  the only reason the rig learned about popups, which we otherwise dismiss on
  sight. `#bug` `#harness`

- **`delegate_xdg_shell!` smuggles in a piece of T6.** Its popup dispatch is
  bounded on `SeatHandler`, so the core now carries an empty `SeatState` and a
  behaviourless trait impl. No `wl_seat` global is advertised (that needs
  `Seat::new`, which is T6's), so nothing about input exists — but it is worth
  saying out loud that the type system pulled a later task's shape forward rather
  than pretending the line stayed clean. `#tradeoff`

- **The rig can now assert *which* error.** Disconnection proves nothing: a
  compositor that kills a client for the wrong reason is still broken. The rig
  reads the connection's protocol-error state (code, interface, message) and every
  error test asserts the code. Built once, reusable forever — the same bet as the
  golden rig. `make test`: 69 tests green (+13), clippy clean; goldens re-blessed:
  none. New golden `xdg_cascade`, verified to reject a one-pixel placement drift.
  `#harness` `#core`

## M1 T6 — input, and a window Roland can actually look at

- **The funnel was the whole design.** One `InputEvent` enum, evdev codes at both
  ends, no Smithay type anywhere near it — and suddenly winit and the test rig are
  the same producer as far as the core is concerned. That means CI exercises the
  production input path rather than a test-shaped imitation of it, and it means
  M2's libinput arrives as a third producer of an existing shape instead of a
  second implementation of input. Everything else in this task was plumbing
  hanging off that one decision. `#input` `#design-decision`

- **The §7 deviation, said out loud.** winit owns the main thread and hands you
  input and presentation through the same loop; there is no arrangement in which a
  nested backend gets a separate T-input. So M1 has T-input's *interface* and not
  its *thread*, and the doc says exactly that: what the pure model wants, what
  winit forces, why it is bounded (protocol objects still have one owner; the
  funnel is already the seam), and what replaces it (libinput + DRM, M2). Writing
  "we deviate here, for this reason, until then" costs a paragraph. Discovering an
  undocumented deviation two milestones later costs a week. `#tradeoff` `#invariant`

- **Roland picked the replica over the query, and I'm glad.** I'd flagged both:
  ask the scene thread per pointer motion (one source of truth, but a synchronous
  cross-thread round-trip on the input path that can queue behind a snapshot) or
  keep a small read-mostly replica on the dispatch thread. §7 already names the
  replica for T-input, and I-2 is explicitly about input not waiting on rendering
  — the query would have been a slow-motion violation of an invariant we wrote
  down ourselves. The replica costs two fields kept in step, in one function.
  `#design-decision` `#invariant`

- **The bug that looked like nothing.** Smithay's pointer API wants the focused
  surface's *origin in global space*; I passed the cursor's *surface-local*
  position. Both are `(f64, f64)`, both type-check, and the result is every
  `enter` reporting (0,0) — which reads like "hit-testing works, coordinates are
  just zero" rather than "you passed the wrong quantity". Fixed by making
  `FocusMap::at` return a `Hit { origin, local }` so the two can never be
  confused again. A tuple was the actual defect; the compiler could not help until
  the names existed. `#bug` `#discovery`

- **The keycode table earns its tradeoff tag.** winit `KeyCode` → evdev is a
  hand-written match covering the standard typing set. Unmapped keys are dropped
  and *counted* — a media key must not kill the compositor, and a silent drop is
  indistinguishable from a broken input path when someone reports "my keyboard
  does nothing". The table is deliberately not exhaustive: M2's libinput hands us
  evdev codes directly and deletes this file rather than growing it. `#tradeoff`

- **CI's "first" system dependency was already there.** The proud comment said no
  apt step, on purpose. Then I ran `ldd` on a test binary: `libxkbcommon.so.0`,
  linked since the protocol layer landed, because Smithay depends on the xkbcommon
  crate unconditionally. CI has been passing on the fact that `ubuntu-latest`
  ships it. So the apt step doesn't *add* a dependency, it converts luck into a
  contract — and the comment now says so instead of claiming a purity we lost
  months ago without noticing. `#discovery` `#ci`

- **What I could and could not verify.** Headlessly: everything except the window
  — 7 input rig tests, 2 socket tests over a real Unix socket with real clients,
  the keycode table, the softbuffer conversion. Then I launched `parhelion-dev`
  for four seconds on this machine: it bound `wayland-3`, printed it, accepted a
  connection, and stayed up. What I cannot check is what the window *looked like*
  — that Roland's eyes have to do. Stating the boundary beats implying I saw
  pixels I never saw. `#backend`

- **A small honest wart.** SIGTERM leaves the socket and lock files behind (no
  handler, so `Drop` never runs); a clean window-close unlinks them properly, and
  wayland-server's lock protocol makes a stale socket harmless on the next bind.
  Noted rather than fixed: signal handling in the dev binary is not M1's business.
  `make test`: 89 tests green (+20), clippy clean, goldens untouched. `#backend`

## M1 T7 — the acceptance run, and the word "no"

- **The milestone nearly ended on a refusal.** Everything M1 built was so a
  terminal nobody wrote for us could connect, map, render, and echo keystrokes.
  `foot` connected, bound our globals, loaded its fonts — and quit: `no clipboard
  available (wl_data_device_manager not implemented by server)`, exit 230. Not a
  warning it degrades past; a hard gate. The prompt had named this exact case in
  advance and told me what to do: stop, report, do not stub. I reported with three
  options and the evidence; Roland chose to implement it properly. Forty lines of
  Smithay wiring later foot started, and the milestone closed on its actual terms
  rather than on a caveat. `#milestone` `#discovery`

- **Not stubbing was the whole test.** It would have taken twenty minutes to
  advertise `wl_data_device_manager` and answer its requests with nothing. foot
  would have started. The window would have appeared. The screenshot would have
  looked like success — and every future client would have believed a compositor
  that says it has a clipboard. An advertised-but-hollow global is not a
  shortcut, it is a lie with a long tail, and the fact that stubbing was *easy
  and would have looked like winning* is exactly why the rule has to be written
  down in advance rather than decided in the moment. `#design-decision`

- **What M1 taught, in one line each.** T1: the narrow line between building 3D
  and forbidding it is an extensible enum with one arm implemented. T2: two
  threads feeding each other queues makes "what if one floods" stop being
  theoretical. T3: the seam held — the frontend/renderer split we bought at M0
  paid at the first real pixels. T4: incremental-equals-from-scratch is the only
  damage property worth having, and it must be *seen* to fail. T5: convenience
  semantics ("composite anything committed") are non-conformance wearing a
  friendly face. T6: winit owns the main thread, so record the deviation instead
  of pretending. T7: the last mile is where you find out whether the previous six
  were honest. `#milestone`

- **The bill for six tasks of discipline came due, and it was small.** foot got
  further than I expected on the first try: registry, globals, shm, fonts, and a
  clean exit path. Nothing in our protocol handling confused it; the one thing it
  wanted, we genuinely do not have. That is the good version of a blocked
  acceptance — a missing feature, not a broken one. `#milestone`

- **Two leftovers closed on the way.** T6's socket litter is gone: signals now set
  a flag, the loop exits through its normal path, and `Drop` unlinks the socket
  and its lock — with a test that spawns the actual binary, signals it, and checks
  the files. That test needed the binary to run without a display, so
  `--headless` exists now; it is the same compositor minus winit, and it is what
  lets CI check a claim that was previously only asserted. `#backend`

- **`wl_output` is the shape of "implement, don't stub" done positively.** Real
  mode from the backend, scale 1 because we implement no scaling, zero physical
  size because a nested window has no millimetres, enter/leave on map and unmap.
  Two of those values are claims M1 cannot fully keep (the refresh rate implies a
  vblank we do not have), so the doc says so rather than letting a future reader
  discover it. `#core`

- **The number that closes M1: 0.62%.** Typing "hello⏎" into a real terminal
  damaged 2 964 pixels of 480 000 — and all 286 pixels that actually changed were
  inside the reported damage region. That is VISION's founding thesis (typing
  redraws a region, not a frame) measured against software we did not write, in a
  test CI re-proves on every push. The whole of M1 — scene graph, snapshots,
  damage algebra, shm, xdg-shell, seat, the funnel — exists to make that one
  number true, and it would have been just as easy to *claim* it. `#milestone`

- **The tripwire earned its keep immediately.** foot throttles on frame callbacks,
  so I sabotaged the notice path and re-ran: the test hung for thirty seconds and
  then said "foot stopped committing after its first frame — frame callbacks are
  not flowing". A terminal that renders once and freezes now cannot pass. That is
  the single most valuable assertion in the suite, and it exists because the
  prompt insisted the acceptance test be *verified to fail*. `#harness`

- **`make test`: 101 tests green**, on the day M1 closed. `#milestone`

## M1 T7b — the compositor learns to refuse honestly, twice

- **Two refusals, one principle, opposite answers.** The clipboard was refused as a
  stub and implemented for real: `foot` would not start without
  `wl_data_device_manager`, and advertising a hollow one would have made it start
  while lying to every client that ever asked for a clipboard. The subcompositor
  went the other way. Same principle — advertise only what you honour — applied to
  a global the whole ecosystem probes for, and the answer came back that
  withdrawing it makes Parhelion a compositor nothing will talk to. Both outcomes
  are the principle working; only one of them is comfortable. `#design-decision`
  `#milestone`

- **The measurement that decided it.** I withdrew `wl_subcompositor` (it *is*
  separable — `subcompositor_global()` + `remove_global`), ran foot, and got
  `err: wayland.c:1746: no sub compositor`, exit 230. Then, before reporting, I
  checked the severity: `WAYLAND_DEBUG=1` shows foot calls `get_subsurface` **zero**
  times in a full session. So the global is a startup probe for foot, not something
  it uses — the gap is real but dormant. That turned "we ship a lie" into "we ship
  a dormant gap with a scheduled fix", which is a different sentence and a
  defensible one. Reporting without that second measurement would have been
  accurate and useless. `#discovery` `#tradeoff`

- **The clipboard bug I only found because the test asked the right question.**
  "A dies while B is focused; does B still hold A's clipboard?" — Smithay clears a
  dead selection lazily, only when the selection is next *sent*, which normally
  means on a focus change. Owner dies, focus unchanged, nobody notices: B keeps an
  offer backed by a corpse. Fixing it needed one more thing I got wrong first —
  the check must run *after* the dying client's teardown, because the surface's
  `destroyed` hook fires while its data source is still alive, and a check there
  cheerfully re-broadcasts the dying offer. Deferred flag, drained at the end of
  the dispatch pass. `#bug` `#core`

- **CI caught a test that was passing for the wrong reason.** My frame-callback
  tripwire waited for foot to commit a *second* frame unprompted. It passed locally
  (the shell prompt happened to arrive after frame one) and failed in CI, where it
  didn't — because an idle terminal with its prompt already drawn has no reason to
  commit anything at all. The fix made the test *cause* the redraw it waits for:
  type, then wait. Stronger claim, no flake, and the sabotage check still trips it.
  A test that only passes when something incidental happens is not a test.
  `#harness` `#bug`

- **And a second flake with a real defect underneath.** One run in eight failed the
  "pixels outside the damage region are unchanged" assertion. Not luck: I compared
  the typing frame against the *settled* frame, so if the terminal repainted across
  two ticks, pixels changed by the first tick sat outside the second tick's damage.
  The comparison has to be frame-versus-*previous*-frame. The flake was the
  assertion being wrong, not the compositor. `#bug` `#harness`

- **wl-copy taught me what the focus gate is for.** I doubted the gate when
  `wl-copy` set the selection with no window on screen — and then traced it: it
  creates an `xdg_toplevel`, waits to be focused, sets the selection, and destroys
  the window, all in about a millisecond. The gate held; real clipboard tooling is
  built around it. (It also explains why waiting for its surface to appear was a
  sampling race — the compositor's own `selections_set` counter is the definite
  condition, and now exists.) `#discovery`

- **Where the clipboard actually lives.** Nowhere in the core. A copy publishes a
  source, a paste asks for a pipe, and the bytes go client to client while the
  compositor holds the introduction. Thirty-nine bytes of UTF-8 crossed between
  `wl-copy` and `wl-paste` in the test, and Parhelion never saw a single one of
  them. `make test`: 108 tests green. `#core` `#milestone`

## M2 T0 — the measurement I got wrong, and the spin that ended

- **I was wrong about foot, and a decision was built on it.** T7b reported that
  foot binds `wl_subcompositor` but never calls `get_subsurface` — the basis for
  T0's whole tripwire design, and for the superseding principle Roland wrote into
  prompt 12. It was a bad grep: `wl_subcompositor@N` where `WAYLAND_DEBUG` prints
  `#`. Empty output, read as evidence of absence. foot in fact creates **nine**
  subsurfaces and puts pixels in **eight** — its decorations. I had even caught
  the same `@`/`#` mistake later in that session while checking wl-copy's focus,
  and never went back to re-run the subsurface count. `#bug` `#discovery`

- **What the correct measurement cost.** Both refusal points were implemented
  before I understood the shape: refuse at `get_subsurface` (foot dies at startup)
  and refuse at "a subsurface commits a buffer" (foot dies moments later, because
  eight of nine carry content). The second one I genuinely believed in for about
  twenty minutes — it is the better rule, refusing exactly where content would be
  dropped, and foot's *first* subsurface is pixel-less, which made it look
  survivable. Nine subsurfaces later, no. There is no refusal point that keeps an
  honest client alive. `#tradeoff`

- **So the debt stands, and it is visible.** foot renders without decorations
  under Parhelion today. That is the silent wrongness the principle forbids, and
  it cannot be contained — only paid, at T7. The useful outcome of T0's first item
  is therefore not a tripwire but an honest test that pins the wrong behaviour and
  inverts when it is fixed, plus a correction entry so nobody rebuilds on the bad
  number. Writing "I measured this wrong and here is what is true" costs less than
  one session; leaving it costs whatever gets built on top. `#milestone`

- **The spin ended by construction, which is the only way it could.** The T2
  promise was "deregister a throttled client's socket". You cannot: wayland-backend
  keeps every client socket inside one epoll fd and hands out no per-client
  registration. The move is to stop asking it — `try_clone` the socket at intake
  and register *our* descriptor with calloop, one source per client. Then throttle
  is `handle.disable(&token)` and there is simply no readiness to report. The
  numbers: old semantics turn the dispatch loop 100 046 times in 300 ms with a
  flood outstanding; the new one turns about fifteen. `#core`

- **Two tempting non-fixes, written down so they stay dead.** Edge-triggering the
  aggregate fd stops the spin and starves shard-mates — the fd never goes quiet
  while a throttled client holds data, so nobody else's readiness produces an edge.
  Timer-based rate limiting bounds the spin without ending it, and would have made
  the "iterations stay bounded" assertion pass while the promise stayed unkept:
  a test certifying the wrong thing is worse than no test. `#design-decision`

- **A pleasant side effect.** Per-client sources are the shape a shard owns. The
  spike asked for an interface where growing to N shards is an implementation
  change; this restructure moved toward it without being asked to. `make test`:
  110 tests green. `#core`

## M2 T7 — the tree, and the debt that took three sessions to pay

- **Three sessions to fix one wrong grep.** T7b measured foot as binding
  `wl_subcompositor` without using it, and built a whole design on that number.
  T0 discovered the number was wrong and could only pin the wrongness in a test.
  T7 implements the feature and inverts that test. The arc is worth remembering
  precisely because the intermediate step — "I cannot fix this, here is the
  measurement, here is the test that will notice when someone can" — is the one
  that felt like failure and turned out to be the load-bearing one. `#milestone`
  `#scene`

- **The children list contains the parent.** That looked like a trick I was
  copying from Smithay until I tried to express `place_below` without it. A child
  can sit *beneath* its parent, so "where the parent is" has to be a position in
  the same list as the children — not an implicit top, not two lists. Smithay
  represents it exactly this way, which made mapping its ordering into ours a
  transcription rather than an interpretation. Borrowing a representation because
  the domain forces it is different from borrowing one because it was there.
  `#scene` `#design-decision`

- **Parent-relative transforms buy the whole moving story.** A child's transform
  is relative to its parent, so moving a window carries its decorations without a
  single child's state changing. Absolute position is computed where it is
  needed — a walk of a chain three deep. I nearly stored absolutes and updated
  them on every parent move; the relative form is smaller, and it makes "the
  parent moved" a one-node change instead of a subtree rewrite. `#scene`

- **76% of the output, from one missing equality check.** Subsurface positions are
  double-buffered and therefore *re-stated on every effective parent commit*. My
  first version damaged old ∪ new unconditionally, which meant every keystroke in
  foot repainted every decoration — 365 354 pixels instead of 2 964. Output
  correct, cost ruinous, and completely invisible without the counters. The
  acceptance test's damage assertion caught it within a minute of subsurfaces
  first working. That number is the whole argument for asserting on counters
  rather than only on pixels. `#core` `#discovery`

- **A HashMap made focus non-deterministic and a clipboard test found it.** Input
  routing now walks trees, and I assigned each surface's stacking index while
  iterating `toplevels` — a HashMap. So which window counted as "topmost" (and
  therefore held keyboard focus) depended on hash order, and one clipboard test
  that maps two windows started failing about half the time. The scene has always
  broken z-ties by `SurfaceId`; the routing table has to do the same, or input and
  pixels quietly disagree about which window you are typing into. `#bug`

- **Two of my own tests were wrong in instructive ways.** One injected pointer
  motion after `commit()` without a round-trip — `commit` only queues, so the
  motion overtook the tree it was meant to be routed against. The other held a
  mouse button down and expected the pointer to cross to another surface; the
  implicit grab is *correct* and my expectation wasn't. Both failures were the
  compositor behaving properly and the test asking the wrong question. `#harness`

- **The renderer did not change.** Not one line. The scene flattens the tree to
  the same back-to-front list it always produced, and the compositor never learns
  that trees exist. That seam was drawn in T1 for a feature nobody had specified
  yet, and it held. `make test`: 124 tests green. foot has decorations.
  `#scene` `#milestone`

## The smoke, run by human eyes (2026-07-26)

- **Two clients, one launched from the other, both decorated, both immediate.**
  Roland ran the smoke: foot has its title bar; his own terminal `rt` runs and
  looks right; starting `rt` *from inside* foot brings up a second window he can
  type into with no perceptible lag. Resize behaves; the cursor over the window is
  clean. The two checklist items that had been marked "eyes only" since T6 are
  finally closed, and the milestone acquired a second witness that nobody wrote
  for it. `#milestone`

- **What the second client is worth.** foot passing is evidence; foot *and* an
  unrelated terminal passing, with one spawning the other, is a different claim —
  it exercises multi-window mapping, focus handover, cascade placement, and two
  independent clients sharing a seat, none of which a single-client test can
  reach. Everything the compositor does for foot it now demonstrably does for
  software it has never met. `#milestone` `#discovery`
