//! The golden comparator: pixel-exact by default, with an explicit,
//! per-test-loosenable tolerance policy.
//!
//! Governing doc: `docs/harness_design.md` (comparator policy). The rule the
//! code enforces: **tolerance and pixel budget default to 0**. CPU-rendered
//! patterns match exactly; the tolerance machinery exists for the GPU future
//! (where rasterisation differs across drivers), and is loosened per-test with
//! a stated reason — never globally.

use parhelion_backend_headless::Frame;

/// A single comparison colour: pixels the comparator flags are painted this in
/// the diff image so they stand out against the dimmed background.
const HIGHLIGHT: [u8; 4] = [255, 0, 255, 255]; // magenta

/// The outcome of comparing an actual frame against a golden.
#[derive(Debug, Clone)]
pub struct CompareResult {
    /// Whether the frames are considered equal under the given policy:
    /// `!size_mismatch && diff_pixel_count <= max_diff_pixels`.
    pub matched: bool,
    /// True when the two frames have different dimensions. When set, no
    /// per-pixel comparison was possible and `diff_image` is `None`.
    pub size_mismatch: bool,
    /// Count of pixels whose largest per-channel absolute difference exceeded
    /// the tolerance.
    pub diff_pixel_count: usize,
    /// The largest per-channel absolute difference seen anywhere (0 when
    /// identical). Useful for choosing a per-test tolerance from evidence.
    pub max_channel_delta: u8,
    /// A visual diff (same dimensions as the inputs): differing pixels painted
    /// magenta, matching pixels dimmed to a third brightness so the highlights
    /// pop. `None` only on a size mismatch.
    pub diff_image: Option<Frame>,
}

/// Compare `actual` against `golden`.
///
/// - `tolerance`: a pixel differs only if its largest per-channel absolute
///   difference is **strictly greater** than this. `0` = bit-exact per channel.
/// - `max_diff_pixels`: how many differing pixels are tolerated before the
///   result is a non-match. `0` = every pixel must match.
///
/// Both default to 0 at the call sites in [`crate::assert_golden`]; a caller
/// that loosens them owes a stated reason (harness policy).
pub fn compare(
    actual: &Frame,
    golden: &Frame,
    tolerance: u8,
    max_diff_pixels: usize,
) -> CompareResult {
    if actual.width() != golden.width() || actual.height() != golden.height() {
        return CompareResult {
            matched: false,
            size_mismatch: true,
            diff_pixel_count: 0,
            max_channel_delta: 0,
            diff_image: None,
        };
    }

    let (w, h) = (actual.width(), actual.height());
    let mut diff = Frame::new(w, h);
    let mut diff_pixel_count = 0usize;
    let mut max_channel_delta = 0u8;

    for y in 0..h {
        for x in 0..w {
            let a = actual.pixel(x, y);
            let g = golden.pixel(x, y);
            // Largest absolute per-channel difference for this pixel.
            let mut pixel_delta = 0u8;
            for c in 0..4 {
                let d = a[c].abs_diff(g[c]);
                pixel_delta = pixel_delta.max(d);
            }
            max_channel_delta = max_channel_delta.max(pixel_delta);

            if pixel_delta > tolerance {
                diff_pixel_count += 1;
                diff.set_pixel(x, y, HIGHLIGHT);
            } else {
                // Dim the actual pixel to a third brightness as context.
                diff.set_pixel(x, y, [a[0] / 3, a[1] / 3, a[2] / 3, 255]);
            }
        }
    }

    CompareResult {
        matched: diff_pixel_count <= max_diff_pixels,
        size_mismatch: false,
        diff_pixel_count,
        max_channel_delta,
        diff_image: Some(diff),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parhelion_backend_headless::test_pattern;

    /// Identical frames match exactly at zero tolerance.
    #[test]
    fn identical_frames_match() {
        let a = test_pattern(32, 24, 0);
        let b = test_pattern(32, 24, 0);
        let r = compare(&a, &b, 0, 0);
        assert!(r.matched);
        assert_eq!(r.diff_pixel_count, 0);
        assert_eq!(r.max_channel_delta, 0);
    }

    /// A single changed pixel is detected at zero tolerance, and appears in the
    /// diff image as the highlight colour.
    #[test]
    fn one_pixel_difference_is_detected() {
        let a = test_pattern(32, 24, 0);
        let mut b = a.clone();
        // Flip a pixel well away from any pattern feature boundary.
        let orig = b.pixel(20, 15);
        b.set_pixel(20, 15, [orig[0] ^ 0xFF, orig[1], orig[2], orig[3]]);

        let r = compare(&a, &b, 0, 0);
        assert!(!r.matched, "one differing pixel must fail at tol=0, budget=0");
        assert_eq!(r.diff_pixel_count, 1);
        assert_eq!(r.diff_image.unwrap().pixel(20, 15), HIGHLIGHT);
    }

    /// Tolerance absorbs a small delta; a larger one still trips.
    #[test]
    fn tolerance_absorbs_small_deltas() {
        let a = test_pattern(32, 24, 0);
        let mut b = a.clone();
        let o = b.pixel(20, 15);
        b.set_pixel(20, 15, [o[0].wrapping_add(3), o[1], o[2], o[3]]);
        // delta 3 is within tolerance 5 → match; within tolerance 2 → not.
        assert!(compare(&a, &b, 5, 0).matched);
        assert!(!compare(&a, &b, 2, 0).matched);
    }

    /// Different sizes are a size mismatch with no diff image.
    #[test]
    fn size_mismatch_reported() {
        let a = test_pattern(32, 24, 0);
        let b = test_pattern(30, 24, 0);
        let r = compare(&a, &b, 0, 0);
        assert!(r.size_mismatch);
        assert!(!r.matched);
        assert!(r.diff_image.is_none());
    }
}
