//! T3 shm end-to-end golden tests: a scripted client draws pixels into a
//! `wl_shm` buffer, attaches, and commits; the dispatch thread copies the buffer
//! into the scene; the CPU compositor paints it over solid background nodes; and
//! the frame is golden-compared.
//!
//! Governing design: `docs/scene_graph_v1.md` §3 (texture-source seam) and the
//! T3 prompt. These exercise the *whole* shm path — client → `wl_shm` → copy at
//! commit → scene → snapshot → blit — with tolerance-0 goldens.
//!
//! Determinism: after `client.roundtrip()` the dispatch thread has already sent
//! the `set_size`/`set_source` mutation into the scene channel (during the commit
//! it processed before the sync reply), so a later `render.tick()` — whose
//! `snapshot()` round-trips the same channel — observes it. No sleeps.
//!
//! To (re)create the goldens after an intended change: `UPDATE_GOLDENS=1 make test`.

use std::os::unix::net::UnixStream;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_headless::Frame;
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::{ClientKey, SceneHandle, SceneThread, SurfaceId, Transform};
use parhelion_harness::assert_golden;
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

/// Frame size for these goldens.
const W: u32 = 32;
const H: u32 = 32;
/// The compositor clears to opaque black before drawing.
const CLEAR: [u8; 4] = [0, 0, 0, 255];
/// Opaque grey background the shm surface composites over.
const BG: [u8; 4] = [80, 80, 80, 255];
/// The shm buffer is a 16×16 patch at the frame origin.
const BUF: u32 = 16;

/// The first protocol-created surface gets id 0; the background is placed
/// directly in the scene with a distinct id/client so nothing collides.
const SHM_SID: SurfaceId = SurfaceId(0);
const BG_SID: SurfaceId = SurfaceId(100);
const BG_CLIENT: ClientKey = ClientKey(9);

/// `wl_shm` bytes (little-endian, so each pixel is `[B, G, R, A]` in memory) for a
/// 2×2 checkerboard with an asymmetric 3×3 magenta marker at the top-left corner
/// — orientation matters, so a flip or transpose is visible. `on_alpha` sets the
/// alpha of the "on" squares (255 for an opaque xrgb buffer; < 255 for an argb
/// buffer whose blend should show).
fn checkerboard(w: u32, h: u32, on_alpha: u8) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b, a) = if x < 3 && y < 3 {
                (255u8, 0u8, 255u8, 255u8) // asymmetric marker (opaque magenta)
            } else if ((x / 2) + (y / 2)) % 2 == 0 {
                (40, 160, 220, on_alpha) // "on" squares (light blue)
            } else {
                (20, 30, 40, 255) // "off" squares (dark, opaque)
            };
            v.extend_from_slice(&[b, g, r, a]);
        }
    }
    v
}

/// A solid `rgb` fill as opaque `wl_shm` bytes (`[B, G, R, 255]` per pixel).
fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        v.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
    }
    v
}

/// Composite the current scene once and return the frame.
fn composite(h: &SceneHandle) -> Frame {
    let mut render = RenderLoop::new(h.clone(), CpuCompositor::new(W, H, CLEAR));
    render.tick(0);
    render.compositor().frame().clone()
}

/// Bring up scene + host + one client, place the grey background, and connect a
/// scripted client. Returns everything the caller needs to drive shm.
fn fixture() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    // Opaque grey background covering the frame, behind the shm surface.
    scene
        .handle()
        .place_solid(BG_SID, BG_CLIENT, Transform::Identity, (W, H), 0, BG, true);
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

/// Put the shm surface above the background (it defaults to z 0, tying with the
/// background; a higher z makes it composite on top).
fn raise_shm(h: &SceneHandle) {
    h.mutate(|s| s.set_z(SHM_SID, 1));
}

/// An opaque XRGB buffer overwrites the background where it draws; the marker and
/// checkerboard land at the origin, the rest stays grey.
#[test]
fn shm_xrgb_composites_over_solid() {
    let (scene, _host, mut client) = fixture();
    let surface = client.create_surface();
    let mut pool = client.create_pool((BUF * BUF * 4) as usize);
    pool.write(&checkerboard(BUF, BUF, 255));
    let buffer = client.create_buffer(&pool, BUF as i32, BUF as i32, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    let h = scene.handle();
    raise_shm(&h);
    let frame = composite(&h);

    // Spot-check the pipeline before trusting the golden.
    assert_eq!(frame.pixel(0, 0), [255, 0, 255, 255], "marker magenta at origin");
    assert_eq!(frame.pixel(4, 4), [40, 160, 220, 255], "opaque 'on' square");
    assert_eq!(frame.pixel(24, 24), BG, "background outside the buffer");
    assert_golden("shm_xrgb", &frame);
}

/// An ARGB buffer blends its translucent "on" squares over the grey background;
/// the marker and "off" squares stay opaque. The blend is visible in the golden.
#[test]
fn shm_argb_blends_over_solid() {
    let (scene, _host, mut client) = fixture();
    let surface = client.create_surface();
    let mut pool = client.create_pool((BUF * BUF * 4) as usize);
    pool.write(&checkerboard(BUF, BUF, 128));
    let buffer = client.create_buffer(&pool, BUF as i32, BUF as i32, ShmFormat::Argb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    let h = scene.handle();
    raise_shm(&h);
    let frame = composite(&h);

    assert_eq!(frame.pixel(0, 0), [255, 0, 255, 255], "marker stays opaque");
    // 50%-alpha [40,160,220] over grey [80,80,80]: integer source-over.
    assert_eq!(frame.pixel(4, 4), [60, 120, 150, 255], "'on' square blended");
    assert_eq!(frame.pixel(24, 24), BG, "background outside the buffer");
    assert_golden("shm_argb", &frame);
}

/// Single-buffer client: after commit the client receives `wl_buffer.release`,
/// then re-draws the *same* buffer and re-commits; the second frame is visible.
/// Proves immediate copy-at-commit release.
#[test]
fn single_buffer_reuse_second_frame_visible() {
    let (scene, _host, mut client) = fixture();
    let surface = client.create_surface();
    let mut pool = client.create_pool((BUF * BUF * 4) as usize);

    // Frame 1: the checkerboard.
    pool.write(&checkerboard(BUF, BUF, 255));
    let buffer = client.create_buffer(&pool, BUF as i32, BUF as i32, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();
    assert_eq!(
        client.buffer_releases(),
        1,
        "buffer released immediately after the first commit"
    );

    // Frame 2: rewrite the SAME buffer's memory to solid green, re-attach it, and
    // commit again — the single-buffer pattern the immediate release enables.
    pool.write(&solid(BUF, BUF, [0, 200, 0]));
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();
    assert_eq!(
        client.buffer_releases(),
        2,
        "same buffer released again after the second commit"
    );

    let h = scene.handle();
    raise_shm(&h);
    let frame = composite(&h);
    assert_eq!(frame.pixel(4, 4), [0, 200, 0, 255], "second frame's green is shown");
    assert_golden("shm_recommit", &frame);
}

/// Destroying the `wl_buffer` after commit is safe: the scene holds an owned copy
/// of the pixels, so the content still composites. (Copy-at-commit makes the
/// buffer's lifetime irrelevant to the scene by construction.)
#[test]
fn destroy_buffer_after_commit_is_safe() {
    let (scene, _host, mut client) = fixture();
    let surface = client.create_surface();
    let mut pool = client.create_pool((BUF * BUF * 4) as usize);
    pool.write(&checkerboard(BUF, BUF, 255));
    let buffer = client.create_buffer(&pool, BUF as i32, BUF as i32, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    // Destroy the buffer the compositor already copied and released.
    buffer.destroy();
    client.roundtrip();

    let h = scene.handle();
    raise_shm(&h);
    let frame = composite(&h);
    // The pixels are still there — the scene's copy outlives the wl_buffer.
    assert_eq!(frame.pixel(0, 0), [255, 0, 255, 255], "marker survives buffer destroy");
    assert_eq!(frame.pixel(4, 4), [40, 160, 220, 255], "content survives buffer destroy");
}
