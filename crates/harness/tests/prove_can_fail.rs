//! Milestone acceptance: prove the golden rig can *fail*.
//!
//! A golden rig that has never been seen to reject a wrong frame proves nothing
//! (CLAUDE.md: "a golden test that was never seen to fail proves nothing").
//! These meta-tests feed the comparator deliberately-wrong frames and assert it
//! reports failure *and* writes the diagnostic artifacts. The meta-tests
//! themselves PASS — so CI stays green while failure detection is demonstrated.

use parhelion_backend_headless::{test_pattern, Frame};
use parhelion_harness::{compare, golden, write_failure_artifacts};

/// A single wrong pixel must be caught at zero tolerance, and the three failure
/// artifacts (actual/golden/diff) must land on disk.
#[test]
fn one_pixel_perturbation_is_caught_with_artifacts() {
    let golden_frame = test_pattern(64, 48, 0);
    let mut actual = golden_frame.clone();
    // Perturb one pixel away from feature boundaries so exactly one differs.
    let o = actual.pixel(30, 20);
    actual.set_pixel(30, 20, [o[0] ^ 0xFF, o[1], o[2], o[3]]);

    let result = compare(&actual, &golden_frame, 0, 0);
    assert!(!result.matched, "one differing pixel must not match at tol=0");
    assert_eq!(result.diff_pixel_count, 1);

    let paths = write_failure_artifacts("__meta_one_pixel", &actual, &golden_frame, result.diff_image.as_ref());
    assert_eq!(paths.len(), 3, "actual + golden + diff artifacts expected");
    for p in &paths {
        assert!(p.exists(), "artifact not written: {}", p.display());
    }
    // Clean up so repeated runs start fresh.
    let _ = std::fs::remove_dir_all(golden::failure_dir("__meta_one_pixel"));
}

/// A whole-image shift by one column trips far more than one pixel — the class
/// of off-by-one/stride bug the grid and corner markers exist to expose.
#[test]
fn shifted_by_one_column_is_caught() {
    let golden_frame = test_pattern(64, 48, 0);
    let shifted = shift_right_one(&golden_frame);

    let result = compare(&shifted, &golden_frame, 0, 0);
    assert!(!result.matched, "a one-column shift must not match");
    assert!(
        result.diff_pixel_count > 1,
        "a shift should differ in many pixels, got {}",
        result.diff_pixel_count
    );
}

/// Copy `src` shifted right by one column; the first column becomes black.
fn shift_right_one(src: &Frame) -> Frame {
    let mut out = Frame::new(src.width(), src.height());
    for y in 0..src.height() {
        for x in 1..src.width() {
            out.set_pixel(x, y, src.pixel(x - 1, y));
        }
    }
    out
}
