//! Scene → snapshot → CPU-compositor golden tests (M1 T1).
//!
//! This is the end-to-end proof of the M1 render path: build canonical scene
//! state on the scene thread, pull an immutable snapshot, composite it into a
//! `Frame`, and golden-compare the pixels. It exercises the whole seam the
//! milestone hangs off — `SceneThread` → `Snapshot` → `RenderLoop` →
//! `CpuCompositor` — with tolerance-0 goldens (CPU rendering is bit-exact;
//! `docs/harness_design.md` §5).
//!
//! Governing design: `docs/scene_graph_v1.md`. To (re)create the goldens after
//! an intended change:  `UPDATE_GOLDENS=1 make test`.
//!
//! What the goldens pin:
//! - `scene_two_overlap` / `scene_two_overlap_restacked` — two overlapping solid
//!   nodes; the only difference between the two is z-order, so they must differ
//!   (stacking is visible, and the comparator distinguishes it).
//! - `scene_clipped` — nodes straddling the frame edges are clipped, not
//!   panicked.
//! - `scene_snapshot_isolation` — a snapshot captured before a mutation renders
//!   as if the mutation never happened (the in-flight-frame isolation property).

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_headless::Frame;
use parhelion_core::render::{Compositor, FrameCounters, RenderLoop};
use parhelion_core::scene::{ClientKey, SceneHandle, SceneThread, SurfaceId, TextureSource, Transform};
use parhelion_harness::assert_golden;

/// Output size for these goldens (matches the M0 test-pattern golden).
const W: u32 = 64;
const H: u32 = 48;
/// The compositor clears to opaque black before drawing (well-defined goldens).
const CLEAR: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [200, 40, 40, 255];
const BLUE: [u8; 4] = [40, 80, 200, 255];

/// Build a scene (via `place`), tick the render loop once, and return the
/// composited frame plus the frame counters.
fn render_scene(place: impl FnOnce(&SceneHandle)) -> (Frame, FrameCounters) {
    let scene = SceneThread::spawn();
    let h = scene.handle();
    place(&h);
    let mut render = RenderLoop::new(h.clone(), CpuCompositor::new(W, H, CLEAR));
    render.tick();
    (render.compositor().frame().clone(), *render.counters())
}

/// Two overlapping opaque nodes stack by z (higher on top); restacking — the
/// *only* change being z — produces a different frame.
#[test]
fn overlapping_solids_stack_by_z_and_restack_changes_output() {
    // Red at z=0 (back), blue at z=1 (front) → blue wins the overlap.
    let (blue_on_top, counters) = render_scene(|h| {
        h.place_solid(SurfaceId(1), ClientKey(0), Transform::Translate { dx: 10, dy: 8 }, (30, 30), 0, RED, true);
        h.place_solid(SurfaceId(2), ClientKey(0), Transform::Translate { dx: 26, dy: 20 }, (30, 30), 1, BLUE, true);
    });
    assert_eq!(counters.frames_produced, 1, "one tick, one frame");
    assert_eq!(counters.nodes_composited, 2, "both nodes composited");
    assert_golden("scene_two_overlap", &blue_on_top);

    // Same nodes, z swapped → red wins the overlap.
    let (red_on_top, _) = render_scene(|h| {
        h.place_solid(SurfaceId(1), ClientKey(0), Transform::Translate { dx: 10, dy: 8 }, (30, 30), 1, RED, true);
        h.place_solid(SurfaceId(2), ClientKey(0), Transform::Translate { dx: 26, dy: 20 }, (30, 30), 0, BLUE, true);
    });
    assert_golden("scene_two_overlap_restacked", &red_on_top);

    // The two frames differ only because of z-order — stacking is real.
    assert_ne!(
        blue_on_top.pixels(),
        red_on_top.pixels(),
        "restacking changed the composited output"
    );
}

/// Nodes straddling the frame edges are clipped to the frame; no panic, and the
/// visible parts land where expected.
#[test]
fn out_of_bounds_nodes_are_clipped() {
    let (frame, counters) = render_scene(|h| {
        // Straddles the top-left corner (negative origin).
        h.place_solid(SurfaceId(1), ClientKey(0), Transform::Translate { dx: -12, dy: -10 }, (30, 30), 0, RED, true);
        // Straddles the bottom-right corner (extends past width/height).
        h.place_solid(SurfaceId(2), ClientKey(0), Transform::Translate { dx: 50, dy: 34 }, (30, 30), 1, BLUE, true);
    });
    assert_eq!(counters.nodes_composited, 2);
    assert_golden("scene_clipped", &frame);
}

/// A snapshot captured before a mutation renders as if the mutation never
/// happened — the in-flight frame is isolated from canonical-state changes.
#[test]
fn in_flight_snapshot_is_isolated_from_later_mutation() {
    let scene = SceneThread::spawn();
    let h = scene.handle();
    h.place_solid(SurfaceId(1), ClientKey(0), Transform::Translate { dx: 8, dy: 8 }, (20, 20), 0, RED, true);

    // Capture, then mutate canonical state: recolour the node and drop a
    // full-frame blue node on top of everything.
    let captured = h.snapshot();
    h.mutate(|s| s.set_source(SurfaceId(1), TextureSource::Solid([0, 200, 0, 255]), true));
    h.place_solid(SurfaceId(2), ClientKey(0), Transform::Identity, (W, H), 5, BLUE, true);

    // Compositing the captured snapshot is unaffected by those mutations.
    let mut comp = CpuCompositor::new(W, H, CLEAR);
    comp.composite(&captured);
    let isolated = comp.frame().clone();

    // A fresh snapshot *does* reflect the mutation (full-frame blue on top).
    let mut fresh = CpuCompositor::new(W, H, CLEAR);
    fresh.composite(&h.snapshot());
    assert_ne!(
        isolated.pixels(),
        fresh.frame().pixels(),
        "the mutation changed canonical state (so isolation is a real property, not a no-op)"
    );

    // The isolated frame is exactly the original red-node-on-black.
    assert_golden("scene_snapshot_isolation", &isolated);
}
