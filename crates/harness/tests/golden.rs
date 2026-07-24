//! The first real golden test: the headless test pattern against its committed
//! golden PNG (`crates/harness/goldens/headless_test_pattern.png`).
//!
//! This is the end-to-end proof that render → compare → pass works. It exercises
//! `assert_golden` at zero tolerance (the pattern is CPU-rendered, so it must
//! match its golden bit-for-bit). To (re)create the golden after an intended
//! change:  `UPDATE_GOLDENS=1 make test`.

use parhelion_backend_headless::test_pattern;
use parhelion_harness::assert_golden;

/// The canonical M0 headless frame matches its committed golden exactly.
#[test]
fn headless_test_pattern_matches_golden() {
    let frame = test_pattern(64, 48, 0);
    assert_golden("headless_test_pattern", &frame);
}
