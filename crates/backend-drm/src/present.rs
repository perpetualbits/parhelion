//! Getting a composited [`Frame`] into a scanout buffer (M2 T1).
//!
//! Governing doc: `docs/scene_graph_v1.md` §13.3 (the frame handoff).
//!
//! Two steps, split because they run on two different threads:
//!
//! 1. [`frame_to_xrgb8888`] — T-render converts its retained RGBA8 frame into
//!    the bytes `DRM_FORMAT_XRGB8888` is defined as. This is the pixel-shaped
//!    work, and it belongs on the thread that already touched every pixel.
//! 2. [`blit_to_pitch`] — T-commit copies those bytes into the dumb buffer's
//!    mapping, row by row, honouring the driver's stride. This is a memcpy per
//!    row and nothing else, on the thread that owns the DRM fd.
//!
//! Both are pure functions over slices, so both are unit-tested in CI on a
//! machine with no GPU — which is the point. The stride handling in particular
//! is the classic silent-corruption bug: get it wrong and the image shears
//! progressively down the screen, which looks like a driver problem and is not.

use parhelion_backend_headless::Frame;

/// Bytes per pixel in `DRM_FORMAT_XRGB8888`.
const BYTES_PER_PIXEL: usize = 4;

/// Convert a [`Frame`]'s RGBA8 bytes into `DRM_FORMAT_XRGB8888` bytes.
///
/// **Channel order, stated once.** `XRGB8888` is defined as a little-endian
/// `0xXXRRGGBB` word, which in memory is the byte sequence `[B, G, R, X]`. Our
/// [`Frame`] stores `[R, G, B, A]`. So this writes bytes directly in the format's
/// own order rather than composing a `u32` — which means the result is correct on
/// a big-endian machine too, for free, instead of being quietly wrong there.
///
/// The `X` byte is written as `0xFF` rather than left at zero. Scanout ignores it
/// for `XRGB8888`, but some drivers and debugging tools alias the buffer as
/// `ARGB8888`, where a zero would mean "fully transparent" and produce a black
/// screen that looks exactly like a mode-setting failure. `0xFF` costs nothing
/// and removes that whole class of confusing evening.
///
/// The frame's own alpha is dropped: the composited frame is already flattened
/// against the compositor's clear colour, and there is nothing behind the screen.
///
/// Writes into `out`, resizing it to fit — the caller recycles one buffer across
/// frames rather than allocating 8 MB per vblank.
pub fn frame_to_xrgb8888(frame: &Frame, out: &mut Vec<u8>) {
    let pixels = frame.pixels();
    out.clear();
    out.reserve(pixels.len());
    for px in pixels.chunks_exact(BYTES_PER_PIXEL) {
        out.extend_from_slice(&[px[2], px[1], px[0], 0xFF]);
    }
}

/// Copy `width × height` tightly-packed `XRGB8888` pixels from `src` into `dst`,
/// a mapped scanout buffer whose rows are `pitch` bytes apart.
///
/// Returns the number of rows actually copied. That is `height` in every ordinary
/// case; a smaller number means `dst` could not hold the image, which the caller
/// reports rather than silently accepting a half-drawn screen.
///
/// # Why the pitch is not `width * 4`
///
/// The kernel picks the stride when it creates a dumb buffer, and it is free to
/// round it up for alignment — 1366 pixels wide is very often a 5504-byte pitch
/// rather than 5464. Copying the source as one flat block would then shear the
/// image by two pixels more on every row down the screen. So the copy is per-row,
/// always, even when the pitch happens to be tight.
///
/// The padding bytes between rows are left untouched: they are never scanned out,
/// and writing them would be a wider memory write for no benefit.
pub fn blit_to_pitch(src: &[u8], width: u32, height: u32, dst: &mut [u8], pitch: u32) -> u32 {
    let row_bytes = (width as usize) * BYTES_PER_PIXEL;
    let pitch = pitch as usize;
    if row_bytes == 0 || pitch < row_bytes {
        // A pitch narrower than a row means the buffer cannot hold this image at
        // all. Copying "as much as fits" would produce a plausible-looking but
        // wrong picture; copying nothing makes the caller's error report the only
        // thing that happens.
        return 0;
    }

    let mut copied = 0;
    for y in 0..height as usize {
        let src_start = y * row_bytes;
        let dst_start = y * pitch;
        // Both bounds checked before slicing: a short `src` or `dst` stops the
        // copy rather than panicking on the frame path (I-1's spirit — the
        // commit thread must not die because a driver surprised us).
        if src_start + row_bytes > src.len() || dst_start + row_bytes > dst.len() {
            break;
        }
        dst[dst_start..dst_start + row_bytes]
            .copy_from_slice(&src[src_start..src_start + row_bytes]);
        copied += 1;
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Channel order is right: pure red, green, and blue land where `XRGB8888`
    /// says they do. A transposition here tints the entire screen, so it gets an
    /// explicit test rather than trust.
    #[test]
    fn channels_land_in_xrgb_byte_order() {
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
        frame_to_xrgb8888(&frame, &mut out);
        assert_eq!(
            out,
            vec![
                0, 0, 255, 0xFF, // red   → B=0,   G=0,   R=255
                0, 255, 0, 0xFF, // green → B=0,   G=255, R=0
                255, 0, 0, 0xFF, // blue  → B=255, G=0,   R=0
            ]
        );
    }

    /// The frame's alpha is dropped and the X byte is opaque — see the function
    /// docs for why `0xFF` rather than zero.
    #[test]
    fn alpha_is_dropped_and_the_x_byte_is_opaque() {
        let frame = Frame::from_rgba(1, 1, vec![10, 20, 30, 0]).expect("well-formed frame");
        let mut out = Vec::new();
        frame_to_xrgb8888(&frame, &mut out);
        assert_eq!(out, vec![30, 20, 10, 0xFF]);
    }

    /// The conversion buffer is reused and re-sized, including when the frame
    /// shrinks — a `clear`-less implementation leaves a stale tail behind.
    #[test]
    fn the_conversion_buffer_is_reused_and_resized() {
        let mut out = Vec::new();
        frame_to_xrgb8888(
            &Frame::from_rgba(2, 2, vec![7; 16]).expect("well-formed frame"),
            &mut out,
        );
        assert_eq!(out.len(), 16);

        frame_to_xrgb8888(
            &Frame::from_rgba(1, 1, vec![1, 2, 3, 255]).expect("well-formed frame"),
            &mut out,
        );
        assert_eq!(out.len(), 4, "a smaller frame shrinks the buffer");
        assert_eq!(out, vec![3, 2, 1, 0xFF]);
    }

    /// The bug this module exists to prevent: a padded stride. Two 2-pixel rows
    /// into a buffer with a 12-byte pitch must land at offsets 0 and 12, with the
    /// four padding bytes untouched.
    #[test]
    fn a_padded_pitch_places_rows_correctly_and_leaves_padding_alone() {
        // 2×2 pixels: row 0 = [A, B], row 1 = [C, D], one byte-value per pixel.
        let src: Vec<u8> = vec![
            1, 1, 1, 1, 2, 2, 2, 2, // row 0
            3, 3, 3, 3, 4, 4, 4, 4, // row 1
        ];
        let mut dst = vec![0xEE; 24]; // 2 rows × 12-byte pitch (8 used, 4 padding)

        let rows = blit_to_pitch(&src, 2, 2, &mut dst, 12);

        assert_eq!(rows, 2);
        assert_eq!(&dst[0..8], &[1, 1, 1, 1, 2, 2, 2, 2]);
        assert_eq!(&dst[8..12], &[0xEE; 4], "row 0's padding is untouched");
        assert_eq!(&dst[12..20], &[3, 3, 3, 3, 4, 4, 4, 4]);
        assert_eq!(&dst[20..24], &[0xEE; 4], "row 1's padding is untouched");
    }

    /// A tight pitch is just the padded case with zero padding — worth pinning,
    /// because it is the case a flat `copy_from_slice` would also get right, and
    /// so the one that hides the bug during development.
    #[test]
    fn a_tight_pitch_copies_contiguously() {
        let src: Vec<u8> = (0..16).collect();
        let mut dst = vec![0; 16];
        assert_eq!(blit_to_pitch(&src, 2, 2, &mut dst, 8), 2);
        assert_eq!(dst, src);
    }

    /// A destination too small copies what it can and reports it; a pitch
    /// narrower than one row copies nothing at all. Neither panics: this runs on
    /// T-commit, and a driver reporting something unexpected must not take the
    /// compositor down with it.
    #[test]
    fn a_short_destination_stops_early_rather_than_panicking() {
        let src: Vec<u8> = vec![0; 32]; // 2×4 pixels
        let mut one_row = vec![0; 8];
        assert_eq!(blit_to_pitch(&src, 2, 4, &mut one_row, 8), 1);

        let mut dst = vec![0xEE; 32];
        assert_eq!(
            blit_to_pitch(&src, 2, 4, &mut dst, 4),
            0,
            "a pitch narrower than a row is refused outright"
        );
        assert_eq!(dst, vec![0xEE; 32], "and nothing is written");
    }
}
