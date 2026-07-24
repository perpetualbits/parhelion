//! Golden storage: encode/decode [`Frame`]s as RGBA8 PNGs, and the on-disk
//! locations goldens and failure artifacts live at.
//!
//! Governing doc: `docs/harness_design.md` (frame/golden format, failure
//! artifact locations). PNGs are written with fixed encoder settings so a
//! blessed golden is byte-stable given the pinned `png` version — the property
//! the "delete + re-bless regenerates byte-identically" determinism check
//! relies on.

use std::fs;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use parhelion_backend_headless::Frame;

/// Directory holding committed goldens: `<harness crate>/goldens/`.
///
/// Anchored at `CARGO_MANIFEST_DIR` so it resolves the same whether tests run
/// from the workspace root or the crate directory.
pub fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

/// Path of a named golden PNG under [`goldens_dir`].
pub fn golden_path(name: &str) -> PathBuf {
    goldens_dir().join(format!("{name}.png"))
}

/// Directory for failure artifacts of a named test:
/// `<workspace>/target/golden-failures/<name>/`.
///
/// Anchored at the workspace `target/` (two levels up from the crate) so all
/// crates drop artifacts in one predictable, git-ignored place.
pub fn failure_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("golden-failures")
        .join(name)
}

/// Encode a frame to a PNG file (RGBA8, 8-bit). Creates parent directories.
///
/// Encoder settings are left at the `png` defaults deliberately: they are fixed
/// for a given crate version, making the output byte-deterministic.
pub fn write_png(path: &Path, frame: &Frame) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), frame.width(), frame.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // png's errors are not io::Error; fold them in so callers handle one type.
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer
        .write_image_data(frame.pixels())
        .map_err(io::Error::other)?;
    Ok(())
}

/// Decode an RGBA8 PNG file into a [`Frame`].
///
/// Rejects anything that is not 8-bit RGBA — goldens are always written that
/// way, so a surprise here means a hand-edited or foreign file, which should
/// fail loudly rather than be silently reinterpreted.
pub fn read_png(path: &Path) -> io::Result<Frame> {
    let file = fs::File::open(path)?;
    let decoder = png::Decoder::new(BufReader::new(file));
    let mut reader = decoder.read_info().map_err(io::Error::other)?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(io::Error::other)?;

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(io::Error::other(format!(
            "golden {} is {:?}/{:?}, expected Rgba/Eight",
            path.display(),
            info.color_type,
            info.bit_depth
        )));
    }
    // `next_frame` may leave trailing capacity; trim to the real frame size.
    buf.truncate(info.buffer_size());

    Frame::from_rgba(info.width, info.height, buf)
        .ok_or_else(|| io::Error::other("decoded PNG size does not match its header"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use parhelion_backend_headless::test_pattern;

    /// A frame survives a PNG encode→decode round-trip bit-for-bit.
    #[test]
    fn png_roundtrip_is_lossless() {
        let frame = test_pattern(48, 32, 0);
        let dir = failure_dir("__golden_roundtrip_selftest");
        let path = dir.join("rt.png");
        write_png(&path, &frame).unwrap();
        let back = read_png(&path).unwrap();
        assert_eq!(frame.pixels(), back.pixels());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Encoding the same frame twice yields byte-identical PNGs — the basis of
    /// the "re-bless regenerates byte-identically" guarantee.
    #[test]
    fn png_encoding_is_deterministic() {
        let frame = test_pattern(48, 32, 0);
        let dir = failure_dir("__golden_determinism_selftest");
        let (p1, p2) = (dir.join("a.png"), dir.join("b.png"));
        write_png(&p1, &frame).unwrap();
        write_png(&p2, &frame).unwrap();
        assert_eq!(fs::read(&p1).unwrap(), fs::read(&p2).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }
}
