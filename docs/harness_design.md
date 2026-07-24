# Parhelion — Test Harness Design

> **Re-entrancy header.**
> **Status:** Draft v0.1 · **Date:** 2026-07-24 · **Kind:** subsystem design (the harness).
> **Upstream:** `parhelion_milestone_plan.md` M0 ("the harness precedes the features"; the golden rig is the velocity multiplier), `CLAUDE.md` ("Test before declaring done"; the prove-it-can-fail rule).
> **Downstream:** every crate that ships a golden test; the protocol rig (M0 task 3b) joins this crate later.
> **Canonical for:** `crates/harness/` and the determinism contract on `crates/backend-headless/`.
> **Scope now:** the golden-screenshot rig (task 3a). The protocol rig (scripted Wayland client → scene assertions) is task 3b and will be documented here when it lands.

---

## 1. Why this exists

Nothing lands in Parhelion without a headless-verifiable test. For anything that
puts pixels on a surface, that test is a **golden**: render a frame to memory,
compare it byte-for-byte against a committed reference image, and fail loudly —
with artifacts — when they differ. This rig is the machinery for that, and it is
built before the features it guards precisely so the features can be built
test-first.

A golden rig is only worth as much as its willingness to reject a wrong frame.
So the rig ships with a **meta-test that proves it can fail** (§6), and every new
golden rig demonstrates a deliberate failure once (`CLAUDE.md`).

## 2. Components and where they live

| Piece | Location | Role |
|-------|----------|------|
| `Frame` | `crates/backend-headless/src/lib.rs` | In-memory image: tightly-packed RGBA8, row-major, top-left origin. `len == w*h*4`, no stride. |
| `test_pattern(w, h, frame)` | same | The M0 CPU-rendered frame producer (§4). Integer-only, deterministic. |
| `compare(...)` + `CompareResult` | `crates/harness/src/compare.rs` | The comparator and its tolerance policy (§5). |
| golden PNG I/O + paths | `crates/harness/src/golden.rs` | Encode/decode RGBA8 PNGs; golden and failure-artifact locations. |
| `assert_golden` / `assert_golden_with` | `crates/harness/src/lib.rs` | The test-facing entry point + blessing workflow (§7). |
| committed goldens | `crates/harness/goldens/<name>.png` | The reference images, in version control. |
| failure artifacts | `target/golden-failures/<name>/` | Written on mismatch; git-ignored (under `/target`). |

The harness depends on `parhelion-backend-headless` for `Frame`. It does **not**
depend on Smithay (spike §5.1) — M0 rendering is ours.

## 3. Frame & golden format

- **Frame:** RGBA8, 8 bits/channel, 4 bytes/pixel, row-major from the top-left,
  tightly packed (no row padding). The tight packing is load-bearing: a stride
  bug shows up as pixel differences instead of being silently absorbed.
- **Golden:** a PNG, colour type **Rgba**, bit depth **Eight**, written with the
  `png` crate's default encoder settings. The decoder rejects anything that is
  not 8-bit RGBA, so a hand-edited or foreign file fails loudly rather than
  being reinterpreted.
- **PNG crate choice:** `png` (pure Rust, C-free) — minimal and keeps CI
  apt-install-free. Recorded in `crates/harness/Cargo.toml`; not load-bearing
  enough for the decision log.

## 4. Determinism requirement (the contract on producers)

Anything that wants a golden test **must** be reproducible byte-for-byte across
machines and runs. Concretely, a golden-tested producer:

- uses **no wall-clock time, no randomness, no uninitialised memory**;
- avoids **architecture-dependent float rounding** — prefer integer math; if
  floats are unavoidable, they must be specified precisely enough to be
  bit-identical everywhere (none are used today);
- depends only on its explicit inputs (for `test_pattern`, that is `w`, `h`,
  `frame`).

`test_pattern` honours this by construction (integer-only). The property is
checked two ways: a unit test asserts two renders are byte-identical, and the
PNG encoder's determinism is asserted directly (`golden.rs` tests). The
end-to-end guarantee — **delete a golden, re-bless, get a byte-identical file** —
was verified by hand for this milestone.

## 5. Comparator policy

`compare(actual, golden, tolerance, max_diff_pixels) -> CompareResult`:

- A pixel **differs** when its largest per-channel absolute difference is
  **strictly greater than `tolerance`**.
- The frames **match** when `diff_pixel_count <= max_diff_pixels` (and sizes are
  equal). A size mismatch is always a non-match.
- `CompareResult` also reports `max_channel_delta` (evidence for choosing a
  tolerance) and a `diff_image` (differing pixels magenta, matching pixels
  dimmed to a third brightness).

**The rule: `tolerance` and `max_diff_pixels` default to 0.** `assert_golden`
compares bit-exactly. CPU-rendered frames match their goldens exactly, so the
tolerance machinery is dormant today — it exists for the GPU future, where
rasterisation legitimately differs across drivers. When that day comes,
tolerance is loosened **per test, with a stated reason** (via
`assert_golden_with`), and **never globally**. A global fudge factor hides real
regressions in every test at once; a per-test one documents exactly which test
tolerates what, and why.

## 6. Prove-it-can-fail

`crates/harness/tests/prove_can_fail.rs` feeds the comparator two
deliberately-wrong frames and asserts it reports failure **and** writes the
`actual` / `golden` / `diff` artifacts:

- a **one-pixel perturbation** (caught at tolerance 0, exactly one differing
  pixel), and
- a **one-column shift** of the whole image (many differing pixels — the
  off-by-one/stride class the grid and corner markers exist to expose).

These meta-tests themselves **pass**, so CI stays green while failure detection
is demonstrated on every run.

## 7. Blessing workflow (the command you actually use)

```text
UPDATE_GOLDENS=1 make test
```

With `UPDATE_GOLDENS=1`, `assert_golden` (re)writes the golden for each golden
test from its actual frame and passes. Use it after an **intended** visual
change, or to create a brand-new golden. Without the flag, a missing golden
fails with a message telling you this exact command.

On a normal (unblessed) mismatch, the failure names three artifacts under
`target/golden-failures/<name>/`:

- `actual.png` — what the producer rendered,
- `golden.png` — the committed reference,
- `diff.png` — differing pixels highlighted magenta.

Review the diff; if the change is intended, re-bless; otherwise, fix the code.

## 8. What is out of scope here (grows later)

- **Protocol rig** (M0 task 3b): scripted Wayland client → scene-state
  assertions. Documented here when it lands.
- **Multi-frame / animation goldens and deterministic dt-trace replay** (M3):
  the `frame` argument to `test_pattern` is the seam, but timeline replay is
  built when the interpolation engine (C7) needs it.
- **Damage / regime-collapse instrumentation counters** (M1/M8): golden tests
  will assert on them, but the counters come with those features.
