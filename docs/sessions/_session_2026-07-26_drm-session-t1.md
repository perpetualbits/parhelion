# Session summary — 2026-07-26 — M2 T1: session, DRM/KMS atomic, dumb buffers

**Task:** `docs/prompts/prompt_14_drm_session.md` (M2 T1). Boot Parhelion from a
TTY: libseat session, atomic KMS commits presenting the existing CPU-rendered
frames through dumb buffers, VT survival, and `wl_output`'s refresh made a
measured fact. The §7 **T-commit** thread is born here.

**Result:** `make test` — **142 tests green, 0 failed**; clippy clean under
`-D warnings`. 17 tests are new. No goldens re-blessed (no rendering behaviour
changed — the DRM backend presents the same frames the CPU compositor already
produced).

---

## What landed

### `crates/backend-drm` — new crate, new workspace member

| File | What it is |
|---|---|
| `Cargo.toml` | `smithay` with `default-features = false, features = ["backend_drm", "backend_session_libseat"]` — **no** renderer, **no** `backend_gbm`, **no** `backend_egl`. The header states which layers are consumed and which are still bypassed. |
| `src/lib.rs` | Crate docs (the thread diagram, the seam, what is deliberately absent), the `Error` type, `OutputMode`, and `run()` — the whole entry point, which spawns both threads and blocks until shutdown. |
| `src/mode.rs` | **Pure**: connector/mode selection policy and the refresh arithmetic, over plain-data structs rather than `drm` types. 8 unit tests. |
| `src/present.rs` | **Pure**: `Frame` → `XRGB8888` conversion and the stride-aware row blit. 6 unit tests. |
| `src/buffer.rs` | `ScanoutBuffer`: dumb buffer + framebuffer object, created/mapped/destroyed through `smithay::reexports::drm`. |
| `src/commit.rs` | **T-commit**: session, device probing, connector scan, CRTC selection, atomic surface, the vblank loop, VT pause/resume, and the watchdog. 1 unit test (candidate device paths). |
| `src/render.rs` | **T-render** on metal: the same `RenderLoop`, ticked by a message from T-commit. |

### Changed elsewhere

| File | Change |
|---|---|
| `crates/core/src/protocol.rs` | `Control::OutputSize(w,h)` → `Control::OutputMode(w,h,refresh_mhz)`; new `ProtocolHost::set_output_mode`; `set_output_size` now delegates to it with the default refresh. `OUTPUT_REFRESH_MHZ`'s doc rewritten: it is now the default for backends *with no vblank*, and explicitly unused on metal. |
| `crates/backend-winit/src/bin/parhelion-dev.rs` | Rewritten arg parsing (`--drm`, `--drm-device PATH`, `--exit-after=SECONDS`, both `--flag value` and `--flag=value` spellings). The DRM branch hands off to the backend, which owns its own render loop because only it knows the screen size. Module docs now carry the first-TTY-run guidance including the two absences. |
| `crates/backend-winit/src/shutdown.rs` | `ShutdownFlag::shared()` — hands out the inner `Arc<AtomicBool>` so the DRM backend polls **the same flag** without depending on this crate. |
| `crates/backend-winit/Cargo.toml` | Depends on `parhelion-backend-drm`, for the *binary* only, with a comment saying so and naming the condition under which `parhelion-dev` earns its own crate. |
| `crates/harness/tests/output.rs` | New test: a backend stating a real refresh (59.953 Hz) has that number reach a client, and it is not the 60 Hz default. |
| `Cargo.toml` | `crates/backend-drm` added to the workspace. |
| `.github/workflows/ci.yml` | `libseat-dev` added, with a header section explaining what CI does and does not run for this crate. |

### Docs

| File | Change |
|---|---|
| `docs/scene_graph_v1.md` | New **§13** (the DRM backend: thread ownership, connector/mode, the frame handoff, VT switching, the seam, what-is-verified-how, what is absent). Old §13 → **§14**. **§9.3** gains the named **re-stated-state rule**. Re-entrancy header updated. |
| `docs/plans/m2_t1_smoke_checklist.md` | **New.** The six-step TTY protocol, every step with its expected outcome and its escape hatch, and the two absences stated in bold before step 1. |
| `docs/parhelion_decision_log.md` | Six entries under "2026-07-26 — Session, DRM/KMS atomic, dumb buffers (M2 T1)". |
| `docs/diary.md` | New section "The metal". |
| `docs/parhelion_project_index.md` | Current state, documents table, subsystems table, and the "future crates" note. |
| `project-map.js` | `drm` node planned → **active** with nine parts; new **`t-commit`** node (done); `render-loop` gains its vblank-driven-tick part; `t-input`'s pending part re-worded now that the DRM backend has landed without it; the stale "current milestone (M1)" strings corrected to M2. `node --check` passes. |

---

## The choices worth knowing about

**No backend trait was added.** The prompt allowed the interface to grow for
vblank-driven ticking. It did not need to: the backends differ only in *who calls
`RenderLoop::tick`*, so the tick became a message. The headless and nested tick
sources are byte-for-byte unchanged, which is why the whole existing suite still
proves what it proved.

**Pixels cross the channel, not `Frame`s.** The prompt's parenthetical put the
copy on T-commit. That is not expressible: the CPU compositor *retains* its frame
for damage tracking (§9.4), so it cannot be moved out from under it, and cloning
it per vblank is a second full-frame copy for nothing. T-render converts to
`XRGB8888` into a recycled buffer — work that has to happen anyway, on the thread
that just touched every pixel — and T-commit does one `copy_from_slice` per row.
Logged.

**Smithay's dumb-buffer wrapper cannot map what it allocates.** `DumbBuffer`
exposes `handle(&self) -> &Handle`; `map_dumb_buffer` needs `&mut`. Four ioctls
through `smithay::reexports::drm` replaced the workaround. Logged.

**libseat's session is `!Send`, and that set the startup sequence.** Both the
session and its notifier are `Rc`-based, so *all* hardware setup happens on
T-commit and the discovered mode travels back over a channel. The upside is real:
`wl_output` is told the truth before the compositor it describes exists, so no
client can ever observe the placeholder size.

**The watchdog, not a retry.** A rejected atomic commit sets "next one is a
modeset" and does nothing else; the loop's 100 ms timeout is the retry. Retrying
in place would be a spin on the one thread that must not spin.

---

## Verification — stated plainly, because this task's acceptance is Roland's eyes

**CI-verified (runs on every push, no hardware):**

- mode-selection policy, including its *total* order (first connected connector;
  preferred mode; else largest area → higher refresh → earlier index; modeless and
  disconnected connectors skipped; nothing connected → `None`);
- refresh arithmetic: an exact 1080p60 mode line, a non-integral panel rate,
  interlace/double-scan/vscan, and degenerate timings refusing to answer;
- `XRGB8888` channel order, alpha dropped, the `X` byte written opaque, buffer
  reuse and shrinking;
- the stride-aware blit: padded pitch (padding untouched), tight pitch, short
  destination clipping, and a too-narrow pitch refused outright;
- a real refresh stated by a backend reaching a client through `wl_output`;
- the crate **builds** in CI. Its hardware paths never execute there.

**Discrimination demonstrated (a test seen to fail):** the refresh test failed on
first run — it asserted 59 952 mHz where the code computes 59 953. The code was
right and my hand arithmetic had rounded the wrong way; the assertion is now the
computed value with the rounding rule pinned. That is the "prove it can fail"
obligation met by accident rather than by design, but it is met.

**NOT verified by anything, deliberately — awaiting the smoke run:**

- that a mode-set happens at all, and that the picture is not sheared;
- that vblanks arrive and the frame cycle sustains itself;
- that a client (`foot`) maps and keeps redrawing on metal;
- that a VT round-trip returns a correct screen in one redraw;
- that shutdown restores the console and leaves no socket litter;
- **what the dev machine's connector actually reports** — connector name, mode,
  refresh to three decimals, and whether the stride is padded. The diary entry
  says outright that it has nothing to record yet and the checklist asks for
  exactly those four facts.

**Absent on metal by construction, and stated in bold in the checklist:** there is
**no input** and **no cursor** until M2 T2. A silent keyboard on a TTY is this
gap, not a regression.

---

## Test count

142 reported by `make test`. **17 are new**: 16 in `parhelion-backend-drm`
(9 × `mode`, 6 × `present`, 1 × `commit`) and 1 in `harness/tests/output.rs`.

The previous session recorded 124, and 124 + 17 = 141. The 142nd is the harness
doctest, which `make test` counts and earlier summaries evidently did not. Noted
rather than quietly reconciled: a test count that drifts by one without
explanation is how a missing test hides.

## Next

**M2 T2** — libinput, the real T-input thread, and the hardware cursor plane. It
retires the §11.2 winit deviation and fills the map's `t-input` seam, and it is
what makes the metal typeable.
