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
