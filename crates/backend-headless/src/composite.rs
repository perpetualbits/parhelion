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
//! - Solid sources only (M1). `Shm` is a declared placeholder (T3); a node
//!   carrying it panics, and none exist in M1.
//! - Integer-only, deterministic, tolerance-0 — it honours the golden
//!   determinism contract (`docs/harness_design.md` §4): no floats, no time, no
//!   randomness. Opaque nodes overwrite; translucent nodes use an integer
//!   source-over blend (documented at [`source_over`]).
//! - No damage/partial-redraw (that is T4): every frame clears and repaints in
//!   full. Damage changes cost, never output.
//!
//! Only [`Transform::Identity`] and [`Transform::Translate`] exist and are
//! handled; there is no reachable transform math beyond integer translation
//! (Thesis 3). When a real transform variant is added it lands with its own
//! composited arm here — until then the `match` is exhaustive over what exists.

use parhelion_core::render::Compositor;
use parhelion_core::scene::{Snapshot, TextureSource, Transform};

use crate::Frame;

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
    fn composite(&mut self, snapshot: &Snapshot) -> usize {
        // Clear the whole frame to the background. Full-frame clear is correct
        // (if wasteful) until damage tracking (T4) scissors it.
        for y in 0..self.frame.height() {
            for x in 0..self.frame.width() {
                self.frame.set_pixel(x, y, self.clear);
            }
        }

        // Draw back-to-front (snapshot order). Each node is a solid rect clipped
        // to the frame; overlap resolves by draw order (later = on top).
        for node in &snapshot.nodes {
            let (ox, oy) = match node.transform {
                Transform::Identity => (0i32, 0i32),
                Transform::Translate { dx, dy } => (dx, dy),
            };
            match node.source {
                TextureSource::Solid(rgba) => {
                    blit_solid(&mut self.frame, ox, oy, node.size, rgba, node.opaque);
                }
                TextureSource::Shm => {
                    unimplemented!("shm source arrives in T3; no Shm nodes exist in M1")
                }
            }
        }

        snapshot.nodes.len()
    }
}

/// Blit a solid `rgba` rectangle whose top-left is at signed screen offset
/// `(ox, oy)` and size `size = (w, h)`, clipped to the frame. Signed offsets and
/// the clip mean a node may sit partly or wholly off-screen without panicking —
/// the out-of-bounds case the acceptance criteria call out. Opaque rectangles
/// overwrite; translucent ones blend via [`source_over`].
fn blit_solid(frame: &mut Frame, ox: i32, oy: i32, size: (u32, u32), rgba: [u8; 4], opaque: bool) {
    let (fw, fh) = (frame.width() as i32, frame.height() as i32);
    // Intersect the node rect [ox, ox+w) × [oy, oy+h) with the frame [0,fw)×[0,fh).
    let x0 = ox.max(0);
    let y0 = oy.max(0);
    let x1 = (ox + size.0 as i32).min(fw);
    let y1 = (oy + size.1 as i32).min(fh);
    // Empty intersection → fully clipped, nothing to draw.
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
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

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// An empty snapshot clears the whole frame to the clear colour and
    /// composites zero nodes.
    #[test]
    fn empty_snapshot_clears_frame() {
        let mut c = CpuCompositor::new(4, 4, BLACK);
        let n = c.composite(&Snapshot::empty());
        assert_eq!(n, 0);
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
        let snap = Snapshot {
            nodes: vec![solid_node(Transform::Translate { dx: 2, dy: 3 }, (4, 5), RED, true)],
        };
        let n = c.composite(&snap);
        assert_eq!(n, 1);
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
        let snap = Snapshot {
            nodes: vec![
                // Drawn first (further back): red covering [0,6)×[0,6).
                solid_node(Transform::Identity, (6, 6), RED, true),
                // Drawn second (on top): blue covering [3,9)×[3,9).
                solid_node(Transform::Translate { dx: 3, dy: 3 }, (6, 6), BLUE, true),
            ],
        };
        assert_eq!(c.composite(&snap), 2);
        assert_eq!(c.frame().pixel(1, 1), RED, "red-only region");
        assert_eq!(c.frame().pixel(8, 8), BLUE, "blue-only region");
        assert_eq!(c.frame().pixel(4, 4), BLUE, "overlap: top (blue) wins");
    }

    /// Nodes crossing the frame edge (negative origin, or extending past it) are
    /// clipped, not panicked; a fully off-screen node draws nothing.
    #[test]
    fn clips_out_of_bounds_nodes() {
        let mut c = CpuCompositor::new(8, 8, BLACK);
        let snap = Snapshot {
            nodes: vec![
                // Straddles the top-left corner: origin (-2,-2), size 5 → visible [0,3)×[0,3).
                solid_node(Transform::Translate { dx: -2, dy: -2 }, (5, 5), RED, true),
                // Entirely off-screen to the right: draws nothing.
                solid_node(Transform::Translate { dx: 20, dy: 0 }, (4, 4), BLUE, true),
            ],
        };
        assert_eq!(c.composite(&snap), 2, "both counted even though one is fully clipped");
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
        let snap = Snapshot {
            nodes: vec![solid_node(Transform::Identity, (2, 2), half_white, false)],
        };
        c.composite(&snap);
        // out = (255*128 + 0*127 + 127)/255 = (32640 + 127)/255 = 32767/255 = 128.
        assert_eq!(c.frame().pixel(0, 0), [128, 128, 128, 255]);
    }
}
