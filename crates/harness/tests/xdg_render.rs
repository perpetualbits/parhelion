//! T5 xdg-shell golden test: two scripted clients map real toplevels and the
//! frame shows them at their C10 cascade placements.
//!
//! Governing design: `docs/scene_graph_v1.md` §10 (mapping semantics) and
//! `CORE-BOUNDARY.md` C10 (default placement is a core fallback until the policy
//! daemon arrives in M4). The scene-level assertion of the same property lives in
//! `xdg.rs::two_toplevels_cascade_deterministically`; this is its pixel form —
//! the golden is what would notice a placement that silently drifted.
//!
//! Determinism: placement is a pure function of toplevel creation order, and the
//! two clients are mapped one after the other on a single dispatch thread, so the
//! order — and therefore the frame — is fixed.
//!
//! To (re)create the golden after an intended change: `UPDATE_GOLDENS=1 make test`.

use std::os::unix::net::UnixStream;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_headless::Frame;
use parhelion_core::protocol::{ProtocolHost, CASCADE_STEP_X, CASCADE_STEP_Y};
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::{SceneHandle, SceneThread};
use parhelion_harness::assert_golden;
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

/// Frame size: wide enough that the second window's cascade offset lands fully
/// inside it, so the offset is visible rather than clipped.
const W: u32 = 96;
const H: u32 = 64;
/// The compositor clears to opaque black before drawing.
const CLEAR: [u8; 4] = [0, 0, 0, 255];
/// Window size for both toplevels.
const WIN_W: i32 = 40;
const WIN_H: i32 = 24;

/// Opaque `wl_shm` bytes (`xrgb8888` → `[B, G, R, X]`) for a `w×h` window filled
/// with `rgb`, with its bottom-right quadrant darkened so the window has an
/// orientation a flip or transpose would betray.
fn window_pixels(w: i32, h: i32, rgb: [u8; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let dark = x >= w / 2 && y >= h / 2;
            let scale = |c: u8| if dark { c / 3 } else { c };
            v.extend_from_slice(&[scale(rgb[2]), scale(rgb[1]), scale(rgb[0]), 255]);
        }
    }
    v
}

/// Composite the current scene once and return the frame.
fn composite(h: &SceneHandle) -> Frame {
    let mut render = RenderLoop::new(h.clone(), CpuCompositor::new(W, H, CLEAR));
    render.tick(0);
    render.compositor().frame().clone()
}

/// Two clients each map one toplevel; the second lands exactly one cascade step
/// down-right of the first, overlapping it. Both windows are visible in the
/// frame, and the later one composites on top.
#[test]
fn two_toplevels_cascade_and_composite() {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());

    // Two separate clients, to show that the cascade is per-compositor (a shared
    // core-side counter), not per-client.
    let (sa, ca) = UnixStream::pair().expect("socketpair a");
    let (sb, cb) = UnixStream::pair().expect("socketpair b");
    host.add_client(sa);
    host.add_client(sb);
    let mut a = ScriptedClient::connect(ca);
    let mut b = ScriptedClient::connect(cb);

    // Map in a fixed order: A takes cascade slot 0, B slot 1. A's round-trip
    // completes before B's first request is sent, so the order is deterministic.
    let _wa = a.map_toplevel(
        WIN_W,
        WIN_H,
        ShmFormat::Xrgb8888,
        &window_pixels(WIN_W, WIN_H, [220, 70, 70]),
    );
    let _wb = b.map_toplevel(
        WIN_W,
        WIN_H,
        ShmFormat::Xrgb8888,
        &window_pixels(WIN_W, WIN_H, [70, 110, 220]),
    );

    let frame = composite(&scene.handle());

    // Spot-check the geometry before trusting the golden: A's top-left corner is
    // at the origin, B's is one cascade step along, and B (mapped second, so
    // later in the tie-broken draw order) wins where they overlap.
    assert_eq!(frame.pixel(0, 0), [220, 70, 70, 255], "A at the cascade origin");
    assert_eq!(
        frame.pixel(CASCADE_STEP_X as u32, CASCADE_STEP_Y as u32),
        [70, 110, 220, 255],
        "B one cascade step down-right, on top of A"
    );
    assert_eq!(
        frame.pixel(W - 1, H - 1),
        CLEAR,
        "background where neither window reaches"
    );

    assert_golden("xdg_cascade", &frame);
}
