//! T4 damage-tracking tests. The one that matters most is **equivalence**:
//! rendering incrementally (retained frame + damage) must be byte-identical to
//! compositing from scratch, at every step of an awkward sequence. Damage may
//! change cost, never output.
//!
//! Governing design: `docs/scene_graph_v1.md` (damage section). Most tests drive
//! the scene directly through `SceneHandle` — the setters and `attach_content`
//! are exactly what the protocol calls — so damage classes (map, move, restack,
//! content update, occluded update, full redraw) can be exercised precisely and
//! deterministically. The copy-on-write isolation test drives a real scripted
//! `wl_shm` client, because that path runs through `build_pixel_block`'s
//! `Arc::make_mut`.

use std::os::unix::net::UnixStream;
use std::sync::Arc;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::render::{Compositor, RenderLoop};
use parhelion_core::scene::{
    ClientKey, ContentDamage, PixelBuffer, ProtocolEvent, Rect, SceneHandle, SceneThread, Snapshot,
    SnapshotDamage, SurfaceId, TextureSource, Transform,
};
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

const W: u32 = 40;
const H: u32 = 30;
const CLEAR: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [200, 40, 40, 255];
const BLUE: [u8; 4] = [40, 80, 200, 255];
const GREEN: [u8; 4] = [40, 200, 40, 255];
const YELLOW: [u8; 4] = [220, 220, 40, 255];
const MAGENTA: [u8; 4] = [200, 40, 200, 255];

/// A solid-colour `w×h` RGBA pixel block.
fn solid(w: u32, h: u32, rgba: [u8; 4]) -> PixelBuffer {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        px.extend_from_slice(&rgba);
    }
    PixelBuffer {
        width: w,
        height: h,
        rgba: px,
    }
}

/// A copy of `prev` with `region` (buffer coords) overwritten with `rgba`. Models
/// a client that redrew only `region` of its buffer — the pixels outside `region`
/// are unchanged, so damage confined to `region` is conservative.
fn patched(prev: &PixelBuffer, region: Rect, rgba: [u8; 4]) -> PixelBuffer {
    let mut px = prev.rgba.clone();
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            let i = ((y as u32 * prev.width + x as u32) * 4) as usize;
            px[i..i + 4].copy_from_slice(&rgba);
        }
    }
    PixelBuffer {
        width: prev.width,
        height: prev.height,
        rgba: px,
    }
}

/// Create a surface and map a block at `(transform, size)` (as the protocol would).
fn map_node(
    h: &SceneHandle,
    sid: SurfaceId,
    client: ClientKey,
    transform: Transform,
    size: (u32, u32),
    block: PixelBuffer,
    opaque: bool,
) {
    h.emit(ProtocolEvent::SurfaceCreated {
        surface: sid,
        client,
    });
    h.mutate(move |s| {
        // Position while invisible (no damage), then map (damages the new extent).
        s.set_geometry(sid, transform, size);
        s.attach_content(sid, size, TextureSource::Shm(Arc::new(block)), opaque, ContentDamage::Full);
    });
}

/// A content update carrying explicit client damage rects (surface coords).
fn update(
    h: &SceneHandle,
    sid: SurfaceId,
    size: (u32, u32),
    block: PixelBuffer,
    opaque: bool,
    rects: Vec<Rect>,
) {
    h.mutate(move |s| {
        s.attach_content(
            sid,
            size,
            TextureSource::Shm(Arc::new(block)),
            opaque,
            ContentDamage::Rects(rects),
        );
    });
}

/// The heart of T4: take the next snapshot, composite it incrementally into the
/// retained `inc`, and compare against a from-scratch full composite of the same
/// nodes. Byte-identical or the test fails, naming the step.
fn assert_equiv(h: &SceneHandle, inc: &mut CpuCompositor, label: &str) {
    let snap = h.snapshot();
    inc.composite(&snap);

    // From-scratch: same nodes, forced full damage, fresh (blank) compositor.
    let full = Snapshot {
        nodes: snap.nodes.clone(),
        damage: SnapshotDamage::Full,
    };
    let mut scratch = CpuCompositor::new(W, H, CLEAR);
    scratch.composite(&full);

    assert_eq!(
        inc.frame().pixels(),
        scratch.frame().pixels(),
        "incremental != from-scratch after: {label}"
    );
}

const A: SurfaceId = SurfaceId(1);
const B: SurfaceId = SurfaceId(2);
const CK: ClientKey = ClientKey(1);

/// Incremental rendering matches from-scratch across an awkward sequence of
/// structural changes and content damage — overlaps, damage spanning two nodes,
/// and damage on an occluded area included.
///
/// Verified to FAIL if the renderer skips honouring a damage class: temporarily
/// making `Scene::set_z` a no-op for damage (drop its `damage_node` call) makes
/// the restack step mismatch. Reverted after checking.
#[test]
fn incremental_equals_from_scratch() {
    let scene = SceneThread::spawn();
    let h = scene.handle();
    let mut inc = CpuCompositor::new(W, H, CLEAR);

    // Step 0: empty first frame (full damage, trivially equal).
    assert_equiv(&h, &mut inc, "empty");

    // Step 1: map A (red 20×20) at the origin, z 0.
    let a0 = solid(20, 20, RED);
    map_node(&h, A, CK, Transform::Identity, (20, 20), a0.clone(), true);
    h.mutate(|s| s.set_z(A, 0));
    assert_equiv(&h, &mut inc, "map A");

    // Step 2: map B (blue 20×20) at (12,6), z 1 — overlaps A.
    let b0 = solid(20, 20, BLUE);
    map_node(&h, B, CK, Transform::Translate { dx: 12, dy: 6 }, (20, 20), b0.clone(), true);
    h.mutate(|s| s.set_z(B, 1));
    assert_equiv(&h, &mut inc, "map B (overlaps A)");

    // Step 3: small content update on A, in a visible (un-occluded) area.
    let a1 = patched(&a0, Rect::new(2, 2, 6, 6), GREEN);
    update(&h, A, (20, 20), a1.clone(), true, vec![Rect::new(2, 2, 6, 6)]);
    assert_equiv(&h, &mut inc, "small update on A (visible)");

    // Step 4: content update on A in an area occluded by B — the damage rect
    // spans both nodes; B must still win on top.
    let a2 = patched(&a1, Rect::new(14, 8, 4, 4), YELLOW);
    update(&h, A, (20, 20), a2, true, vec![Rect::new(14, 8, 4, 4)]);
    assert_equiv(&h, &mut inc, "occluded update on A (under B)");

    // Step 5: move B to (4,4) — structural: damages old ∪ new extent.
    h.mutate(|s| s.set_geometry(B, Transform::Translate { dx: 4, dy: 4 }, (20, 20)));
    assert_equiv(&h, &mut inc, "move B");

    // Step 6: restack A above B — the overlap flips who is on top.
    h.mutate(|s| s.set_z(A, 5));
    assert_equiv(&h, &mut inc, "restack A above B");

    // Step 7: full source swap on B (same extent, ContentDamage::Full — the
    // conservative case the protocol sends when a commit carries no usable
    // client damage).
    let magenta = solid(20, 20, MAGENTA);
    h.mutate(move |s| {
        s.attach_content(
            B,
            (20, 20),
            TextureSource::Shm(Arc::new(magenta)),
            true,
            ContentDamage::Full,
        );
    });
    assert_equiv(&h, &mut inc, "full source swap on B");

    // Step 8: explicit full redraw (the conservative fallback).
    h.mutate(|s| s.damage_full());
    assert_equiv(&h, &mut inc, "full redraw");
}

/// A small-damage content update on a large surface redraws pixels proportional
/// to the damage area, not the whole frame — demonstrated by the counters.
#[test]
fn small_damage_redraws_a_small_region() {
    // Damage area is 10×10 = 100 px. The bound is generous — 4× the damage area
    // — to allow rect granularity and any conservative rounding, while staying
    // orders of magnitude below a full 100×100 = 10_000-px frame.
    const DAMAGE_AREA: u64 = 100;
    const BOUND: u64 = DAMAGE_AREA * 4;

    let scene = SceneThread::spawn();
    let h = scene.handle();
    let base = solid(100, 100, RED);
    map_node(&h, A, CK, Transform::Identity, (100, 100), base.clone(), true);

    let mut render = RenderLoop::new(h.clone(), CpuCompositor::new(100, 100, CLEAR));
    render.tick(0); // first frame: full damage, whole surface painted
    let after_map = render.counters().pixels_redrawn;

    // Patch a 10×10 region and damage exactly it.
    let up = patched(&base, Rect::new(30, 30, 10, 10), GREEN);
    update(&h, A, (100, 100), up, true, vec![Rect::new(30, 30, 10, 10)]);
    render.tick(0);
    let delta = render.counters().pixels_redrawn - after_map;

    assert!(delta >= DAMAGE_AREA, "redrew at least the damage ({delta} < {DAMAGE_AREA})");
    assert!(delta <= BOUND, "redraw stayed proportional to damage ({delta} > {BOUND})");
    assert!(delta < 10_000, "redraw was far less than a full frame ({delta})");
}

/// Many scattered damage rects collapse (region coalescing) but the output is
/// still correct — equivalence holds through the collapse.
#[test]
fn many_scattered_rects_coalesce_and_stay_correct() {
    let scene = SceneThread::spawn();
    let h = scene.handle();
    let mut inc = CpuCompositor::new(W, H, CLEAR);

    let base = solid(30, 20, RED);
    map_node(&h, A, CK, Transform::Identity, (30, 20), base.clone(), true);
    assert_equiv(&h, &mut inc, "map A");

    // Scatter far more than the coalesce threshold of 1×1 damage rects, patching
    // exactly those pixels. The region collapses to a bounding box (over-
    // approximate); since the block is unchanged outside the patched pixels,
    // repainting the bbox is still correct.
    let mut patch = base.clone();
    let mut rects = Vec::new();
    for i in 0..60i32 {
        let (x, y) = (i % 30, (i * 7) % 20);
        patch = patched(&patch, Rect::new(x, y, 1, 1), GREEN);
        rects.push(Rect::new(x, y, 1, 1));
    }
    update(&h, A, (30, 20), patch, true, rects);
    assert_equiv(&h, &mut inc, "coalesced many-rects update");
}

/// Copy-on-write isolation: a snapshot taken before a partial-copy commit keeps
/// its pixels byte-for-byte, because `build_pixel_block` clones the shared block
/// (`Arc::make_mut`) before patching. Driven through the real protocol path.
#[test]
fn partial_copy_does_not_mutate_in_flight_snapshot() {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let mut client = ScriptedClient::connect(client_end);

    // Map an 8×8 all-white buffer.
    let surface = client.create_surface();
    let mut pool = client.create_pool(8 * 8 * 4);
    pool.write(&vec![255u8; 8 * 8 * 4]); // xrgb: [B,G,R,X] all 255 → white
    let buffer = client.create_buffer(&pool, 8, 8, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    let h = scene.handle();
    // Capture a snapshot that shares the surface's pixel block by Arc.
    let captured = h.snapshot();
    let mut c1 = CpuCompositor::new(8, 8, CLEAR);
    c1.composite(&captured);
    let before = c1.frame().pixels().to_vec();

    // Partial-copy commit: rewrite only a 4×4 corner to red, damage just it, and
    // re-attach the same buffer. build_pixel_block CoW-clones the shared block.
    let mut px = vec![255u8; 8 * 8 * 4];
    for y in 0..4 {
        for x in 0..4 {
            let i = (y * 8 + x) * 4;
            px[i..i + 4].copy_from_slice(&[0, 0, 255, 255]); // xrgb [B,G,R,X] → red
        }
    }
    pool.write(&px);
    client.damage(&surface, 0, 0, 4, 4);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    // The captured snapshot, recomposited, is unchanged — its Arc was not mutated.
    let mut c2 = CpuCompositor::new(8, 8, CLEAR);
    c2.composite(&captured);
    assert_eq!(before, c2.frame().pixels(), "in-flight snapshot's pixels were mutated");

    // A fresh snapshot does reflect the update (so the commit really happened).
    let mut c3 = CpuCompositor::new(8, 8, CLEAR);
    c3.composite(&h.snapshot());
    assert_ne!(
        before,
        c3.frame().pixels().to_vec(),
        "the partial-copy update was not visible in a fresh snapshot"
    );
}
