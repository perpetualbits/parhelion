//! CPU compositor v1 — paints an immutable scene [`Snapshot`] into a [`Frame`].
//!
//! Governing design: `docs/scene_graph_v1.md` and `docs/CORE-BOUNDARY.md` §3 C5
//! (the render loop's "record" step) / C10 (solid-colour fallbacks). This is the
//! M1 implementor of the core's [`Compositor`] seam
//! ([`parhelion_core::render::Compositor`]); the core drives it without ever
//! naming [`Frame`]. It evolves the M0 `test_pattern` code into something driven
//! by scene state instead of a fixed pattern.
//!
//! # What it does (and deliberately does not)
//!
//! - Clears to a fixed colour, then draws each snapshot node **back-to-front**
//!   (the snapshot is pre-sorted; we iterate as-is — painter's algorithm).
//! - Two source kinds (M1): `Solid` colours and `Shm` pixel blocks (T3). Both go
//!   through the same clip + integer-blend path; the compositor knows nothing
//!   about "shm" — it blits whatever [`PixelBuffer`] the snapshot hands it (the
//!   texture-source seam, `docs/scene_graph_v1.md` §3).
//! - Integer-only, deterministic, tolerance-0 — it honours the golden
//!   determinism contract (`docs/harness_design.md` §4): no floats, no time, no
//!   randomness. Opaque nodes overwrite; translucent nodes use an integer
//!   source-over blend (documented at [`source_over`]).
//! - **Retained-frame damage rendering (T4):** the frame persists between
//!   composites; each tick recomputes only within the snapshot's damage — per
//!   rect, clear it and redraw the nodes intersecting it, back-to-front. Pixels
//!   outside damage keep their retained values. A `Full`-damage snapshot repaints
//!   everything (first frame / fallback). Because damage is conservative,
//!   incremental output is byte-identical to a from-scratch full repaint — the
//!   governing property, exercised by the equivalence test in `harness`.
//!
//! Only [`Transform::Identity`] and [`Transform::Translate`] exist and are
//! handled; there is no reachable transform math beyond integer translation
//! (Thesis 3). When a real transform variant is added it lands with its own
//! composited arm here — until then the `match` is exhaustive over what exists.

use parhelion_core::render::{CompositeStats, Compositor};
use parhelion_core::scene::{PixelBuffer, Rect, Snapshot, SnapshotDamage, TextureSource, Transform};

use crate::Frame;

/// The output offset a node's transform resolves to (identity/translate only in
/// M1). Also the surface→output origin.
fn node_offset(transform: Transform) -> (i32, i32) {
    match transform {
        Transform::Identity => (0, 0),
        Transform::Translate { dx, dy } => (dx, dy),
    }
}

/// A CPU compositor that owns one in-memory [`Frame`] and repaints it from a
/// [`Snapshot`] on each `composite`. The frame is read back with [`Self::frame`]
/// — that is how the render loop (which never sees `Frame`) lets a test fetch
/// the pixels to golden-compare.
pub struct CpuCompositor {
    /// The render target, repainted in full each `composite`.
    frame: Frame,
    /// The colour every frame starts from before nodes are drawn, `[r,g,b,a]`.
    clear: [u8; 4],
}

impl CpuCompositor {
    /// A `width × height` compositor that clears to `clear` before each frame.
    /// The initial frame is unpainted (all zero) until the first `composite`.
    pub fn new(width: u32, height: u32, clear: [u8; 4]) -> Self {
        CpuCompositor {
            frame: Frame::new(width, height),
            clear,
        }
    }

    /// The current frame — the result of the most recent `composite` (or a blank
    /// frame before any). Borrowed for golden comparison; never mutated here.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }
}

impl Compositor for CpuCompositor {
    fn composite(&mut self, snapshot: &Snapshot) -> CompositeStats {
        let (fw, fh) = (self.frame.width() as i32, self.frame.height() as i32);
        let frame_rect = Rect::new(0, 0, fw, fh);

        match &snapshot.damage {
            // Full damage: repaint the whole frame (first frame / fallback).
            SnapshotDamage::Full => {
                let nodes = self.paint_rect(snapshot, frame_rect);
                CompositeStats {
                    nodes_composited: nodes,
                    pixels_redrawn: frame_rect.area(),
                }
            }
            // Region damage: repaint only the damaged rects over the retained
            // frame. Pixels outside every rect keep their previous values.
            SnapshotDamage::Region(region) => {
                let mut nodes = 0;
                let mut pixels = 0;
                for rect in region.rects() {
                    let clip = rect.intersect(&frame_rect);
                    if clip.is_empty() {
                        continue;
                    }
                    pixels += clip.area();
                    nodes += self.paint_rect(snapshot, clip);
                }
                CompositeStats {
                    nodes_composited: nodes,
                    pixels_redrawn: pixels,
                }
            }
        }
    }
}

impl CpuCompositor {
    /// Repaint one clip rect (already intersected with the frame): clear it to the
    /// background, then draw every node intersecting it, back-to-front, each
    /// clipped to `clip`. Returns the number of nodes it drew. This is the unit of
    /// damage rendering — a `Full` frame is just this over the whole frame rect.
    fn paint_rect(&mut self, snapshot: &Snapshot, clip: Rect) -> usize {
        let clear = self.clear;
        clear_rect(&mut self.frame, clip, clear);

        let mut nodes = 0;
        for node in &snapshot.nodes {
            let (ox, oy) = node_offset(node.transform);
            let node_rect = Rect::new(ox, oy, node.size.0 as i32, node.size.1 as i32);
            // Skip nodes that don't touch this clip rect — they cost nothing.
            if node_rect.intersect(&clip).is_empty() {
                continue;
            }
            nodes += 1;
            match &node.source {
                TextureSource::Solid(rgba) => {
                    blit_solid(&mut self.frame, ox, oy, node.size, *rgba, node.opaque, clip);
                }
                TextureSource::Shm(pixels) => {
                    blit_pixels(&mut self.frame, ox, oy, pixels, node.opaque, clip);
                }
            }
        }
        nodes
    }
}

/// Fill `clip` (assumed already within the frame) with `color`.
fn clear_rect(frame: &mut Frame, clip: Rect, color: [u8; 4]) {
    for y in clip.y..clip.bottom() {
        for x in clip.x..clip.right() {
            frame.set_pixel(x as u32, y as u32, color);
        }
    }
}

/// Blit a solid `rgba` rectangle whose top-left is at signed screen offset
/// `(ox, oy)` and size `size = (w, h)`, clipped to `clip` (which the caller has
/// already intersected with the frame). Signed offsets and the clip mean a node
/// may sit partly or wholly off-screen without panicking. Opaque rectangles
/// overwrite; translucent ones blend via [`source_over`].
fn blit_solid(
    frame: &mut Frame,
    ox: i32,
    oy: i32,
    size: (u32, u32),
    rgba: [u8; 4],
    opaque: bool,
    clip: Rect,
) {
    // Draw region = node rect ∩ clip (clip is already within the frame).
    let draw = Rect::new(ox, oy, size.0 as i32, size.1 as i32).intersect(&clip);
    if draw.is_empty() {
        return;
    }
    for y in draw.y..draw.bottom() {
        for x in draw.x..draw.right() {
            let (ux, uy) = (x as u32, y as u32);
            let out = if opaque {
                rgba
            } else {
                source_over(rgba, frame.pixel(ux, uy))
            };
            frame.set_pixel(ux, uy, out);
        }
    }
}

/// Blit a decoded [`PixelBuffer`] whose top-left is at signed screen offset
/// `(ox, oy)`, clipped to the frame. Same shape as [`blit_solid`] but the colour
/// varies per pixel (read from `buf`, which is tightly-packed RGBA8). `opaque`
/// (from the node — set by the copy path: `xrgb8888` → opaque, `argb8888` →
/// blend) selects overwrite vs integer [`source_over`]. The renderer neither
/// knows nor cares that these bytes came from `wl_shm`.
fn blit_pixels(frame: &mut Frame, ox: i32, oy: i32, buf: &PixelBuffer, opaque: bool, clip: Rect) {
    // Draw region = buffer rect ∩ clip (clip is already within the frame).
    let draw = Rect::new(ox, oy, buf.width as i32, buf.height as i32).intersect(&clip);
    if draw.is_empty() {
        return;
    }
    for y in draw.y..draw.bottom() {
        for x in draw.x..draw.right() {
            // Source pixel in buffer-local coordinates (in-bounds because the draw
            // region is within the buffer rect, so bx < width and by < height).
            let bx = (x - ox) as usize;
            let by = (y - oy) as usize;
            let i = (by * buf.width as usize + bx) * 4;
            let src = [buf.rgba[i], buf.rgba[i + 1], buf.rgba[i + 2], buf.rgba[i + 3]];
            let (ux, uy) = (x as u32, y as u32);
            let out = if opaque {
                src
            } else {
                source_over(src, frame.pixel(ux, uy))
            };
            frame.set_pixel(ux, uy, out);
        }
    }
}

/// Integer source-over blend of `src` over `dst`, both straight (non-premultiplied)
/// RGBA. Per colour channel: `out = (src·a + dst·(255−a) + 127) / 255`, with the
/// `+127` giving round-to-nearest instead of truncation. Output alpha composites
/// the same way (`a + dst_a·(255−a)/255`). All integer arithmetic — no float
/// rounding, so it is bit-identical across machines (the golden determinism
/// contract, `docs/harness_design.md` §4). For a fully opaque source (`a = 255`)
/// this reduces to `out = src` exactly, which is why the opaque path can skip it.
fn source_over(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let a = src[3] as u32;
    let inv = 255 - a;
    let chan = |s: u8, d: u8| (((s as u32 * a) + (d as u32 * inv) + 127) / 255) as u8;
    let out_a = (a + (dst[3] as u32 * inv + 127) / 255).min(255) as u8;
    [
        chan(src[0], dst[0]),
        chan(src[1], dst[1]),
        chan(src[2], dst[2]),
        out_a,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use parhelion_core::scene::SnapshotNode;

    /// Build a one-node snapshot for the common test shape.
    fn solid_node(transform: Transform, size: (u32, u32), rgba: [u8; 4], opaque: bool) -> SnapshotNode {
        SnapshotNode {
            transform,
            size,
            source: TextureSource::Solid(rgba),
            opaque,
        }
    }

    /// Wrap nodes in a full-damage snapshot — these unit tests all paint from a
    /// blank frame, so full damage means "paint everything" (the from-scratch
    /// path). Region-damage behaviour is exercised end-to-end in `harness`.
    fn full(nodes: Vec<SnapshotNode>) -> Snapshot {
        Snapshot {
            nodes,
            damage: SnapshotDamage::Full,
        }
    }

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// An empty snapshot clears the whole frame to the clear colour and
    /// composites zero nodes.
    #[test]
    fn empty_snapshot_clears_frame() {
        let mut c = CpuCompositor::new(4, 4, BLACK);
        let n = c.composite(&Snapshot::empty());
        assert_eq!(n.nodes_composited, 0);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(c.frame().pixel(x, y), BLACK, "pixel ({x},{y}) cleared");
            }
        }
    }

    /// A single opaque node fills exactly its rect; outside it stays clear.
    #[test]
    fn single_solid_node_fills_its_rect() {
        let mut c = CpuCompositor::new(10, 10, BLACK);
        let snap = full(vec![solid_node(Transform::Translate { dx: 2, dy: 3 }, (4, 5), RED, true)]);
        let n = c.composite(&snap);
        assert_eq!(n.nodes_composited, 1);
        // Inside [2,6) × [3,8) is red.
        assert_eq!(c.frame().pixel(2, 3), RED);
        assert_eq!(c.frame().pixel(5, 7), RED);
        // Just outside stays clear.
        assert_eq!(c.frame().pixel(1, 3), BLACK);
        assert_eq!(c.frame().pixel(6, 3), BLACK);
        assert_eq!(c.frame().pixel(2, 8), BLACK);
    }

    /// Two overlapping opaque nodes: in the overlap the later (back-to-front)
    /// node wins — this is the stacking order goldens exercise.
    #[test]
    fn stacks_back_to_front_top_wins() {
        let mut c = CpuCompositor::new(10, 10, BLACK);
        let snap = full(vec![
                // Drawn first (further back): red covering [0,6)×[0,6).
                solid_node(Transform::Identity, (6, 6), RED, true),
                // Drawn second (on top): blue covering [3,9)×[3,9).
                solid_node(Transform::Translate { dx: 3, dy: 3 }, (6, 6), BLUE, true),
            ]);
        assert_eq!(c.composite(&snap).nodes_composited, 2);
        assert_eq!(c.frame().pixel(1, 1), RED, "red-only region");
        assert_eq!(c.frame().pixel(8, 8), BLUE, "blue-only region");
        assert_eq!(c.frame().pixel(4, 4), BLUE, "overlap: top (blue) wins");
    }

    /// Nodes crossing the frame edge (negative origin, or extending past it) are
    /// clipped, not panicked; a fully off-screen node draws nothing.
    #[test]
    fn clips_out_of_bounds_nodes() {
        let mut c = CpuCompositor::new(8, 8, BLACK);
        let snap = full(vec![
                // Straddles the top-left corner: origin (-2,-2), size 5 → visible [0,3)×[0,3).
                solid_node(Transform::Translate { dx: -2, dy: -2 }, (5, 5), RED, true),
                // Entirely off-screen to the right: draws nothing.
                solid_node(Transform::Translate { dx: 20, dy: 0 }, (4, 4), BLUE, true),
            ]);
        // The off-screen node is skipped (it touches no damage), so only the
        // on-screen node is composited — damage/clip culling in action.
        assert_eq!(c.composite(&snap).nodes_composited, 1, "only the on-screen node is composited");
        assert_eq!(c.frame().pixel(0, 0), RED, "clipped node's visible part drew");
        assert_eq!(c.frame().pixel(2, 2), RED);
        assert_eq!(c.frame().pixel(3, 3), BLACK, "past the clipped node");
        // The off-screen blue node left no trace anywhere.
        for y in 0..8 {
            for x in 0..8 {
                assert_ne!(c.frame().pixel(x, y), BLUE, "off-screen node drew at ({x},{y})");
            }
        }
    }

    /// A translucent node blends over what is beneath it via the integer
    /// source-over formula. 50%-alpha white over black → mid-grey (128).
    #[test]
    fn translucent_node_blends_over_background() {
        let mut c = CpuCompositor::new(2, 2, BLACK);
        let half_white = [255, 255, 255, 128];
        let snap = full(vec![solid_node(Transform::Identity, (2, 2), half_white, false)]);
        c.composite(&snap);
        // out = (255*128 + 0*127 + 127)/255 = (32640 + 127)/255 = 32767/255 = 128.
        assert_eq!(c.frame().pixel(0, 0), [128, 128, 128, 255]);
    }

    /// Build a one-node snapshot from a pixel block (the shm shape).
    fn pixels_node(
        transform: Transform,
        buf: PixelBuffer,
        opaque: bool,
    ) -> SnapshotNode {
        let size = (buf.width, buf.height);
        SnapshotNode {
            transform,
            size,
            source: TextureSource::Shm(std::sync::Arc::new(buf)),
            opaque,
        }
    }

    /// An opaque (xrgb-style) pixel block overwrites per-pixel and is clipped to
    /// the frame — the shm blit path, exercising per-pixel colour and clipping.
    #[test]
    fn opaque_pixel_block_blits_and_clips() {
        let mut c = CpuCompositor::new(4, 4, BLACK);
        // 2×2 block: distinct colours per pixel so orientation is observable.
        // Row-major, top-left origin: [TL, TR, BL, BR] = red, green, blue, white.
        let buf = PixelBuffer {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, // (0,0) red
                0, 255, 0, 255, // (1,0) green
                0, 0, 255, 255, // (0,1) blue
                255, 255, 255, 255, // (1,1) white
            ],
        };
        let snap = full(vec![pixels_node(Transform::Translate { dx: 1, dy: 1 }, buf, true)]);
        assert_eq!(c.composite(&snap).nodes_composited, 1);
        assert_eq!(c.frame().pixel(1, 1), [255, 0, 0, 255], "TL red at (1,1)");
        assert_eq!(c.frame().pixel(2, 1), [0, 255, 0, 255], "TR green at (2,1)");
        assert_eq!(c.frame().pixel(1, 2), [0, 0, 255, 255], "BL blue at (1,2)");
        assert_eq!(c.frame().pixel(2, 2), [255, 255, 255, 255], "BR white at (2,2)");
        assert_eq!(c.frame().pixel(0, 0), BLACK, "outside the block stays clear");
    }

    /// A translucent (argb-style) pixel block blends over what is beneath it,
    /// per pixel, via the same integer source-over as solids.
    #[test]
    fn translucent_pixel_block_blends() {
        let mut c = CpuCompositor::new(1, 1, BLACK);
        // Single 50%-alpha white pixel over black → mid-grey (128), same formula
        // as the solid translucent case.
        let buf = PixelBuffer {
            width: 1,
            height: 1,
            rgba: vec![255, 255, 255, 128],
        };
        let snap = full(vec![pixels_node(Transform::Identity, buf, false)]);
        c.composite(&snap);
        assert_eq!(c.frame().pixel(0, 0), [128, 128, 128, 255]);
    }
}
