//! Parhelion headless backend — in-memory rendering for deterministic tests.
//!
//! Governing doc: `docs/parhelion_milestone_plan.md` M0 (the headless backend
//! renders a test pattern to memory, feeding the golden-screenshot rig) and
//! `docs/harness_design.md` (the determinism contract every golden-tested
//! producer must honour). The backend catalogue lives in the subsystem table
//! in `CLAUDE.md`.
//!
//! # Scope (M0 task 3a)
//!
//! This crate is deliberately tiny: a [`Frame`] (RGBA8 pixels in memory) and a
//! [`test_pattern`] that draws one. It is **not** a renderer architecture.
//! There are no traits anticipating M1's real renderer, no scene-graph types,
//! and no GPU — M1 replaces the internals and designs the seam it needs then,
//! with the scene graph on the table (CLAUDE.md: no abstractions beyond what
//! the task requires).
//!
//! # Determinism
//!
//! Everything here is integer-only and free of time, randomness, and
//! architecture-dependent float rounding. Two calls with the same arguments
//! produce byte-identical pixels on any machine — the property the golden rig
//! depends on (`docs/harness_design.md`).

// The CPU compositor v1 (M1): paints a scene `Snapshot` from `parhelion-core`
// into a `Frame`. Lives here — with the `Frame` it renders into — so the core
// depends on no backend (see the crate/module docs and `crates/core/render.rs`).
pub mod composite;

/// An in-memory image: tightly-packed 8-bit RGBA, row-major, top-left origin.
///
/// The pixel buffer length is always exactly `width * height * 4`. There is no
/// stride/padding — "tightly packed" is a load-bearing guarantee the golden
/// comparator relies on, so stride bugs surface as pixel differences rather
/// than being silently absorbed.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
    /// RGBA8 pixels, row-major, `width * height * 4` bytes.
    pixels: Vec<u8>,
}

impl Frame {
    /// Create a `width × height` frame with every channel zeroed (transparent
    /// black). Panics if `width * height * 4` overflows `usize` — a test-only
    /// concern, and a panic is the honest response to an impossible frame.
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|wh| wh.checked_mul(4))
            .expect("frame dimensions overflow usize");
        Frame {
            width,
            height,
            pixels: vec![0; len],
        }
    }

    /// Build a frame from an existing tightly-packed RGBA8 buffer (e.g. one
    /// decoded from a golden PNG). Returns `None` if `pixels.len()` is not
    /// exactly `width * height * 4`, so a malformed golden fails loudly rather
    /// than comparing against garbage.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
        if pixels.len() != expected {
            return None;
        }
        Some(Frame {
            width,
            height,
            pixels,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw RGBA8 buffer (`width * height * 4` bytes).
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Byte offset of pixel `(x, y)` in the buffer. Caller guarantees bounds.
    fn offset(&self, x: u32, y: u32) -> usize {
        // Row-major: (y * width + x) * 4. All operands fit because x < width
        // and y < height, and the product fit at construction time.
        ((y as usize * self.width as usize) + x as usize) * 4
    }

    /// Read pixel `(x, y)` as `[r, g, b, a]`. Panics if out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "pixel out of bounds");
        let o = self.offset(x, y);
        [
            self.pixels[o],
            self.pixels[o + 1],
            self.pixels[o + 2],
            self.pixels[o + 3],
        ]
    }

    /// Write pixel `(x, y)` from `[r, g, b, a]`. Panics if out of bounds.
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        assert!(x < self.width && y < self.height, "pixel out of bounds");
        let o = self.offset(x, y);
        self.pixels[o..o + 4].copy_from_slice(&rgba);
    }
}

impl std::fmt::Debug for Frame {
    /// Debug prints dimensions and byte count, never the whole buffer — a
    /// 64×48 frame is 12 KiB and dumping it helps no one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

/// Draw the M0 test pattern into a fresh `width × height` [`Frame`].
///
/// The pattern is designed to *exercise a comparator*, not to look pretty. Each
/// element targets a class of bug a golden rig must catch:
///
/// - **Two-axis gradient background** — smooth ramps (integer division), the
///   place per-channel tolerance would first matter for the GPU future.
/// - **1-pixel grid + a hard rectangle edge** — off-by-one and stride bugs turn
///   a crisp grid into a smear or a shear.
/// - **Four distinct corner markers** (TL red, TR green, BL blue, BR yellow) —
///   any orientation flip or row/column transpose swaps them.
/// - **A solid reference patch of exact colour** `#804020` — a known constant
///   that must round-trip through PNG encode/decode bit-for-bit.
/// - **A frame cursor**: a 1-pixel vertical bar at `x = frame % width`, so the
///   `frame` argument has a real, deterministic effect (multi-frame use is not
///   faked). The committed golden uses `frame = 0`, putting the bar at `x = 0`.
///
/// Integer-only and deterministic (see the module docs).
pub fn test_pattern(width: u32, height: u32, frame: u32) -> Frame {
    let mut f = Frame::new(width, height);
    if width == 0 || height == 0 {
        return f;
    }

    // Denominators for the gradient. Guard the 1-pixel dimension so we never
    // divide by zero; a single-column/row image gets a flat channel there.
    let wm1 = width.saturating_sub(1).max(1);
    let hm1 = height.saturating_sub(1).max(1);

    for y in 0..height {
        for x in 0..width {
            // Background: red ramps along x, green ramps along y, constant blue.
            // (x * 255) / (width-1) is exact integer math — no float rounding.
            let r = (x * 255 / wm1) as u8;
            let g = (y * 255 / hm1) as u8;
            let b = 64u8;
            f.set_pixel(x, y, [r, g, b, 255]);

            // 1-pixel grid every 16 px: a hard edge the comparator must keep
            // crisp. White lines on the gradient.
            if x % 16 == 0 || y % 16 == 0 {
                f.set_pixel(x, y, [255, 255, 255, 255]);
            }
        }
    }

    // Solid reference patch of an exact colour, centred. Sized to fit even
    // small frames (clamped below), so the constant is always present.
    let patch = [0x80, 0x40, 0x20, 0xFF];
    let pw = (width / 4).clamp(1, width);
    let ph = (height / 4).clamp(1, height);
    let px0 = width / 2 - pw / 2;
    let py0 = height / 2 - ph / 2;
    for y in py0..(py0 + ph).min(height) {
        for x in px0..(px0 + pw).min(width) {
            f.set_pixel(x, y, patch);
        }
    }

    // Corner markers: 4×4 blocks (clamped), each a distinct primary so a flip
    // or transpose is obvious.
    let m = 4u32.min(width).min(height);
    fill_block(&mut f, 0, 0, m, m, [255, 0, 0, 255]); // top-left: red
    fill_block(&mut f, width - m, 0, m, m, [0, 255, 0, 255]); // top-right: green
    fill_block(&mut f, 0, height - m, m, m, [0, 0, 255, 255]); // bottom-left: blue
    fill_block(&mut f, width - m, height - m, m, m, [255, 255, 0, 255]); // bottom-right: yellow

    // Frame cursor: a vertical bar whose column is a function of `frame`.
    let cx = frame % width;
    for y in 0..height {
        f.set_pixel(cx, y, [255, 0, 255, 255]); // magenta
    }

    f
}

/// Fill a `w × h` block with top-left corner `(x0, y0)`, clipped to the frame.
fn fill_block(f: &mut Frame, x0: u32, y0: u32, w: u32, h: u32, rgba: [u8; 4]) {
    for y in y0..(y0 + h).min(f.height()) {
        for x in x0..(x0 + w).min(f.width()) {
            f.set_pixel(x, y, rgba);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buffer is exactly `w*h*4` bytes and accessors round-trip.
    #[test]
    fn frame_is_tightly_packed() {
        let mut f = Frame::new(3, 2);
        assert_eq!(f.pixels().len(), 3 * 2 * 4);
        f.set_pixel(2, 1, [1, 2, 3, 4]);
        assert_eq!(f.pixel(2, 1), [1, 2, 3, 4]);
    }

    /// Determinism: identical arguments produce byte-identical frames.
    #[test]
    fn test_pattern_is_deterministic() {
        let a = test_pattern(64, 48, 0);
        let b = test_pattern(64, 48, 0);
        assert_eq!(a.pixels(), b.pixels());
    }

    /// The exact reference colour is present at the frame centre.
    #[test]
    fn reference_patch_is_exact() {
        let f = test_pattern(64, 48, 0);
        assert_eq!(f.pixel(32, 24), [0x80, 0x40, 0x20, 0xFF]);
    }

    /// The frame cursor moves with the `frame` argument.
    #[test]
    fn frame_cursor_tracks_frame_arg() {
        let f0 = test_pattern(64, 48, 0);
        let f5 = test_pattern(64, 48, 5);
        assert_eq!(f0.pixel(0, 10), [255, 0, 255, 255]); // bar at x=0
        assert_eq!(f5.pixel(5, 10), [255, 0, 255, 255]); // bar at x=5
    }
}
