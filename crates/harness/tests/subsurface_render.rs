//! M2 T7 subsurface goldens: the tree, composited.
//!
//! Governing design: `docs/scene_graph_v1.md` (the subsurface tree section).
//! These pin what the conformance tests can only count: that a child lands at the
//! right *pixels*, in the right order, at the right moment.
//!
//! The sync-atomicity pair is the one that earns its keep. A synchronized child's
//! commit must not be visible until its parent commits, and "not visible" is a
//! claim about a frame, not about a node count — so both frames are pinned: the
//! one before the parent's commit (old content) and the one after (new).
//!
//! To (re)create the goldens after an intended change: `UPDATE_GOLDENS=1 make test`.

use std::os::unix::net::UnixStream;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_headless::Frame;
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::{SceneHandle, SceneThread};
use parhelion_harness::assert_golden;
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

const W: u32 = 64;
const H: u32 = 48;
const CLEAR: [u8; 4] = [0, 0, 0, 255];

/// Parent window size, and the child patch size.
const PARENT: i32 = 40;
const CHILD: i32 = 16;

/// `wl_shm` bytes (`xrgb8888` → `[B, G, R, X]`) for a solid `w×h` block.
fn solid(w: i32, h: i32, rgb: [u8; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        v.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
    }
    v
}

fn composite(h: &SceneHandle) -> Frame {
    let mut render = RenderLoop::new(h.clone(), CpuCompositor::new(W, H, CLEAR));
    render.tick(0);
    render.compositor().frame().clone()
}

fn fixture() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

/// A child above its parent, offset — the ordinary case, and the shape of every
/// toolkit's decoration.
#[test]
fn subsurface_above_parent_composites_at_its_offset() {
    let (scene, _host, mut client) = fixture();

    let win = client.map_toplevel(
        PARENT,
        PARENT,
        ShmFormat::Xrgb8888,
        &solid(PARENT, PARENT, [60, 70, 90]),
    );
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.set_subsurface_position(&sub, 8, 6);
    client.draw(&child, CHILD, CHILD, &solid(CHILD, CHILD, [220, 120, 40]));
    client.commit(&win.surface);
    client.roundtrip();

    let frame = composite(&scene.handle());
    // Spot-check before trusting the golden: the child's colour at its offset,
    // the parent's outside it.
    assert_eq!(frame.pixel(8, 6), [220, 120, 40, 255], "child's top-left corner");
    assert_eq!(frame.pixel(2, 2), [60, 70, 90, 255], "parent shows around it");
    assert_golden("subsurface_above", &frame);
}

/// A child placed **below** its parent: visible only where the parent is not, so
/// an offset child peeks out from underneath.
#[test]
fn subsurface_below_parent_is_hidden_where_the_parent_covers_it() {
    let (scene, _host, mut client) = fixture();

    let win = client.map_toplevel(
        PARENT,
        PARENT,
        ShmFormat::Xrgb8888,
        &solid(PARENT, PARENT, [60, 70, 90]),
    );
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    // Offset so part of it lies beyond the parent's edge.
    client.set_subsurface_position(&sub, 32, 32);
    client.draw(&child, CHILD, CHILD, &solid(CHILD, CHILD, [220, 120, 40]));
    client.place_below(&sub, &win.surface);
    client.commit(&win.surface);
    client.roundtrip();

    let frame = composite(&scene.handle());
    assert_eq!(
        frame.pixel(34, 34),
        [60, 70, 90, 255],
        "under the parent, the parent wins"
    );
    assert_eq!(
        frame.pixel(44, 44),
        [220, 120, 40, 255],
        "beyond the parent's edge, the child shows"
    );
    assert_golden("subsurface_below", &frame);
}

/// A child of a child: offsets accumulate down the tree, and the scene must not
/// assume depth 1.
#[test]
fn nested_subsurfaces_accumulate_their_offsets() {
    let (scene, _host, mut client) = fixture();

    let win = client.map_toplevel(
        PARENT,
        PARENT,
        ShmFormat::Xrgb8888,
        &solid(PARENT, PARENT, [60, 70, 90]),
    );
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.set_subsurface_position(&sub, 6, 6);
    client.draw(&child, CHILD, CHILD, &solid(CHILD, CHILD, [220, 120, 40]));

    let grandchild = client.create_surface();
    let sub2 = client.get_subsurface(&grandchild, &child);
    client.set_subsurface_position(&sub2, 4, 4);
    client.draw(&grandchild, 6, 6, &solid(6, 6, [40, 200, 120]));

    client.commit(&win.surface);
    client.roundtrip();

    let frame = composite(&scene.handle());
    assert_eq!(
        frame.pixel(10, 10),
        [40, 200, 120, 255],
        "grandchild at 6+4 = 10: offsets accumulate through the tree"
    );
    assert_eq!(frame.pixel(7, 7), [220, 120, 40, 255], "child around it");
    assert_golden("subsurface_nested", &frame);
}

/// **The sync-atomicity pair.** A synchronized child commits new content; until
/// the parent commits, the frame must be unchanged. Both frames are pinned,
/// because "nothing appeared yet" is a claim about pixels.
#[test]
fn sync_child_content_appears_only_when_the_parent_commits() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let win = client.map_toplevel(
        PARENT,
        PARENT,
        ShmFormat::Xrgb8888,
        &solid(PARENT, PARENT, [60, 70, 90]),
    );
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.set_subsurface_position(&sub, 8, 6);
    client.draw(&child, CHILD, CHILD, &solid(CHILD, CHILD, [220, 120, 40]));
    client.commit(&win.surface);
    client.roundtrip();

    // Now the child commits *different* content, and the parent stays quiet.
    client.draw(&child, CHILD, CHILD, &solid(CHILD, CHILD, [40, 200, 120]));
    client.roundtrip();

    let before = composite(&h);
    assert_eq!(
        before.pixel(10, 8),
        [220, 120, 40, 255],
        "the old content is still what composites — the child's commit is cached"
    );
    assert_golden("subsurface_sync_before_parent_commit", &before);

    // The parent commits: the child's cached state becomes current.
    client.commit(&win.surface);
    client.roundtrip();

    let after = composite(&h);
    assert_eq!(
        after.pixel(10, 8),
        [40, 200, 120, 255],
        "and now the new content is there"
    );
    assert_golden("subsurface_sync_after_parent_commit", &after);
}
