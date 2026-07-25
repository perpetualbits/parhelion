//! Presenting a composited [`Frame`] in a desktop window (M1 T6).
//!
//! The CPU compositor paints into a [`Frame`] — tightly-packed RGBA8, the
//! headless backend's format and the one the goldens are written in. softbuffer
//! wants a `u32` per pixel in the host's native order. This module is that one
//! conversion, kept separate from the winit event plumbing so it can be unit
//! tested without a display.

use parhelion_backend_headless::Frame;

/// Convert a [`Frame`]'s RGBA8 bytes into softbuffer's pixel format.
///
/// **Channel order, stated once.** softbuffer takes each pixel as a `u32` laid
/// out `0x00RRGGBB` — red in bits 16–23, green in 8–15, blue in 0–7, top byte
/// unused. Our `Frame` stores `[R, G, B, A]` bytes in memory. So the conversion
/// is a shift-and-or per pixel, and the **alpha channel is dropped**: the window
/// is opaque, the composited frame is already flattened against its clear
/// colour, and there is nothing behind it to blend with.
///
/// Writes into `out`, resizing it to match — the caller reuses one buffer across
/// frames rather than allocating per present.
pub fn frame_to_argb(frame: &Frame, out: &mut Vec<u32>) {
    let pixels = frame.pixels();
    out.clear();
    out.reserve(pixels.len() / 4);
    out.extend(pixels.chunks_exact(4).map(|px| {
        let (r, g, b) = (px[0] as u32, px[1] as u32, px[2] as u32);
        (r << 16) | (g << 8) | b
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Channel order is right: pure red, green, and blue land in the bytes the
    /// window expects. A transposition here would tint the whole desktop, so it
    /// gets an explicit test rather than trust.
    #[test]
    fn channels_land_in_the_right_bits() {
        let frame = Frame::from_rgba(
            3,
            1,
            vec![
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
            ],
        )
        .expect("well-formed frame");

        let mut out = Vec::new();
        frame_to_argb(&frame, &mut out);
        assert_eq!(out, vec![0x00FF_0000, 0x0000_FF00, 0x0000_00FF]);
    }

    /// Alpha is dropped, not blended: a half-transparent pixel presents as its
    /// own colour, because the window has nothing behind it.
    #[test]
    fn alpha_is_dropped() {
        let frame = Frame::from_rgba(1, 1, vec![10, 20, 30, 128]).expect("well-formed frame");
        let mut out = Vec::new();
        frame_to_argb(&frame, &mut out);
        assert_eq!(out, vec![0x000A_141E]);
    }

    /// The output buffer is reused across frames — including when the frame gets
    /// smaller, which a `clear`-less implementation would leave stale pixels in.
    #[test]
    fn the_buffer_is_reused_and_resized() {
        let mut out = Vec::new();
        let big = Frame::from_rgba(2, 2, vec![7; 16]).expect("well-formed frame");
        frame_to_argb(&big, &mut out);
        assert_eq!(out.len(), 4);

        let small = Frame::from_rgba(1, 1, vec![1, 2, 3, 255]).expect("well-formed frame");
        frame_to_argb(&small, &mut out);
        assert_eq!(out.len(), 1, "shrinking frames shrink the buffer");
        assert_eq!(out[0], 0x0001_0203);
    }
}
