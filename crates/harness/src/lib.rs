//! Parhelion test harness — headless golden-screenshot rig (M0 task 3a).
//!
//! Governing docs: `docs/harness_design.md` (this rig's canonical design) and
//! `docs/parhelion_milestone_plan.md` M0 ("nothing lands without a
//! headless-verifiable test"; the golden rig is the velocity multiplier).
//!
//! # What this rig does
//!
//! [`assert_golden`] renders → compares a [`Frame`] against a committed golden
//! PNG, and on mismatch writes the actual frame, the golden, and a visual diff
//! to `target/golden-failures/<name>/` before failing with all three paths.
//!
//! # Two rules that make goldens trustworthy
//!
//! 1. **Exact by default.** Tolerance and pixel budget default to 0
//!    ([`assert_golden`]); CPU-rendered frames match bit-for-bit. Loosening is
//!    per-test via [`assert_golden_with`], with a stated reason — never global.
//! 2. **Determinism is the producer's job.** Anything golden-tested must be
//!    reproducible byte-for-byte (see `parhelion_backend_headless`); otherwise
//!    the golden is noise.
//!
//! # Blessing workflow
//!
//! ```text
//! UPDATE_GOLDENS=1 make test
//! ```
//!
//! rewrites the golden for every failing (or missing) golden test, then the
//! tests pass. This is the command to reach for after an intentional visual
//! change. It is honoured by [`assert_golden`]/[`assert_golden_with`] via the
//! `UPDATE_GOLDENS` environment variable.
//!
//! The protocol rig ([`protocol_rig`]) drives a real
//! [`parhelion_core::protocol::ProtocolHost`] with a scripted in-process
//! Wayland client, asserting on wire behaviour and the core's scene ledger.

pub mod compare;
pub mod golden;
pub mod protocol_rig;

use std::path::PathBuf;

pub use compare::{compare, CompareResult};
pub use parhelion_backend_headless::Frame;

/// Whether the blessing mode is active (`UPDATE_GOLDENS=1`).
fn update_goldens_enabled() -> bool {
    std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")
}

/// Assert that `actual` matches the committed golden named `name`, bit-exactly.
///
/// Zero tolerance, zero pixel budget. See [`assert_golden_with`] for the
/// loosenable variant. Panics (fails the test) on mismatch, missing golden, or
/// I/O error, after writing failure artifacts where applicable.
pub fn assert_golden(name: &str, actual: &Frame) {
    assert_golden_with(name, actual, 0, 0);
}

/// Like [`assert_golden`] but with an explicit `tolerance` (max per-channel
/// delta absorbed) and `max_diff_pixels` (differing-pixel budget).
///
/// Use only with a stated reason in the calling test — the harness policy is
/// that tolerance is justified per-test, never applied globally
/// (`docs/harness_design.md`).
pub fn assert_golden_with(name: &str, actual: &Frame, tolerance: u8, max_diff_pixels: usize) {
    let golden_path = golden::golden_path(name);

    // Blessing mode: (re)write the golden from the actual frame and pass.
    if update_goldens_enabled() {
        golden::write_png(&golden_path, actual)
            .unwrap_or_else(|e| panic!("failed to bless golden '{name}': {e}"));
        eprintln!("[golden] blessed '{name}' -> {}", golden_path.display());
        return;
    }

    // No golden yet, and not blessing: tell the user exactly how to create it.
    if !golden_path.exists() {
        panic!(
            "golden '{name}' does not exist at {}.\n\
             Create it with:  UPDATE_GOLDENS=1 make test",
            golden_path.display()
        );
    }

    let golden = golden::read_png(&golden_path)
        .unwrap_or_else(|e| panic!("failed to read golden '{name}' at {}: {e}", golden_path.display()));

    let result = compare(actual, &golden, tolerance, max_diff_pixels);
    if result.matched {
        return;
    }

    // Mismatch: persist artifacts and fail naming every path.
    let paths = write_failure_artifacts(name, actual, &golden, result.diff_image.as_ref());
    let artifacts = paths
        .iter()
        .map(|p| format!("  {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    let detail = if result.size_mismatch {
        format!(
            "size mismatch: actual {}x{}, golden {}x{}",
            actual.width(),
            actual.height(),
            golden.width(),
            golden.height()
        )
    } else {
        format!(
            "{} differing pixel(s), max channel delta {} (tolerance {}, budget {})",
            result.diff_pixel_count, result.max_channel_delta, tolerance, max_diff_pixels
        )
    };
    panic!(
        "golden '{name}' mismatch: {detail}\nartifacts:\n{artifacts}\n\
         If this change is intended, re-bless with:  UPDATE_GOLDENS=1 make test"
    );
}

/// Write failure artifacts for test `name` into `target/golden-failures/<name>/`
/// and return the paths written. `diff` is `None` on a size mismatch (no
/// per-pixel diff exists). Public so the prove-it-can-fail meta-test can assert
/// the artifacts appear.
pub fn write_failure_artifacts(
    name: &str,
    actual: &Frame,
    golden: &Frame,
    diff: Option<&Frame>,
) -> Vec<PathBuf> {
    let dir = golden::failure_dir(name);
    let mut written = Vec::new();

    let actual_path = dir.join("actual.png");
    golden::write_png(&actual_path, actual).expect("write actual.png");
    written.push(actual_path);

    let golden_path = dir.join("golden.png");
    golden::write_png(&golden_path, golden).expect("write golden.png");
    written.push(golden_path);

    if let Some(diff) = diff {
        let diff_path = dir.join("diff.png");
        golden::write_png(&diff_path, diff).expect("write diff.png");
        written.push(diff_path);
    }

    written
}
