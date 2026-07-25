//! T7 conformance sweep: the error paths of the globals Parhelion advertises.
//!
//! Governing design: `docs/plans/m1_tasks.md` T7 ("conformance sweep for all
//! implemented globals"). The other conformance tests live with the feature they
//! belong to — xdg-shell's three protocol errors in `xdg.rs`, the frame-callback
//! lifecycle in `protocol.rs`, the seat's delivery rules in `input.rs`, the
//! output's advertisement in `output.rs`. This file holds what the sweep found
//! *missing*: the `wl_shm` rejection path, and `xdg_output`'s logical geometry.
//!
//! The standard every one of them meets: assert the **specific** error, never
//! merely that the client was disconnected. A compositor that kills a client for
//! the wrong reason is still broken.

use std::os::unix::net::UnixStream;

use parhelion_core::protocol::{ProtocolHost, DEFAULT_OUTPUT_SIZE};
use parhelion_core::scene::{SceneThread, SurfaceId, Transform};
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

fn fixture() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

/// `wl_shm.error.invalid_stride` — the code the compositor posts when a buffer's
/// claimed geometry does not fit the pool backing it. (The errors are declared on
/// `wl_shm`; they are *posted* on the `wl_shm_pool` object that could not satisfy
/// the request.)
const WL_SHM_ERROR_INVALID_STRIDE: u32 = 1;

/// A buffer whose rows do not fit its pool is rejected — not accepted, and
/// *certainly* not read out of bounds.
///
/// This is the hostile-input boundary in miniature: the pool is client-owned
/// shared memory, the geometry is a client-supplied claim about it, and the
/// compositor's job is to disbelieve the claim. Parhelion's own copy path has a
/// second guard of its own (`build_pixel_block` re-checks stride and offset
/// against the mapping before reading a byte), so this is defence in depth — but
/// the protocol-level rejection is what a client is entitled to.
#[test]
fn a_buffer_that_does_not_fit_its_pool_is_rejected() {
    let (_scene, _host, mut client) = fixture();

    // A 16×16 pool, then a buffer claiming rows four times as wide as they are.
    let pool = client.create_pool(16 * 16 * 4);
    let _buffer = client.create_buffer_raw(&pool, 0, 16, 16, 16 * 16, ShmFormat::Xrgb8888);

    let err = client.expect_protocol_error();
    assert_eq!(
        err.interface, "wl_shm_pool",
        "the error is posted on the pool that could not satisfy the request"
    );
    assert_eq!(
        err.code, WL_SHM_ERROR_INVALID_STRIDE,
        "wl_shm.error.invalid_stride ({})",
        err.message
    );
}

/// A buffer offset past the end of its pool is rejected by the same guard — the
/// case an out-of-bounds read would otherwise fall straight through.
#[test]
fn a_buffer_offset_past_the_pool_is_rejected() {
    let (_scene, _host, mut client) = fixture();

    let pool = client.create_pool(16 * 16 * 4);
    // Offset a whole pool's worth past the start.
    let _buffer = client.create_buffer_raw(&pool, 16 * 16 * 4, 4, 4, 16, ShmFormat::Xrgb8888);

    let err = client.expect_protocol_error();
    assert_eq!(err.interface, "wl_shm_pool");
    assert_eq!(
        err.code, WL_SHM_ERROR_INVALID_STRIDE,
        "out-of-range geometry is rejected with the same code ({})",
        err.message
    );
}

/// The registry advertises exactly the set of globals Parhelion means to serve.
///
/// **On `wl_subcompositor`:** it is advertised *ahead of its support*, which the
/// governing decision permits on one condition — every unsupported request on it
/// must refuse loudly (see `asking_for_a_subsurface_is_refused_out_loud`).
/// Withdrawing it instead was tried and measured: `foot` then refuses to start
/// (`no sub compositor`, exit 230), because clients hard-gate on the presence of
/// globals they never use. The assertion below pins *what is advertised*, so any
/// future change to that set is deliberate rather than a side effect.
#[test]
fn the_registry_advertises_exactly_the_expected_globals() {
    let (_scene, _host, client) = fixture();
    let globals: Vec<String> = client
        .advertised_globals()
        .into_iter()
        .map(|(interface, _)| interface)
        .collect();

    for expected in [
        "wl_compositor",
        "wl_shm",
        "wl_seat",
        "wl_output",
        "xdg_wm_base",
        "wl_data_device_manager",
        "zxdg_output_manager_v1",
    ] {
        assert!(
            globals.iter().any(|g| g == expected),
            "{expected} is advertised (globals: {globals:?})"
        );
    }

    // Pinned deliberately, debt and all: changing what we advertise should be a
    // decision, not a side effect of a dependency bump.
    assert!(
        globals.iter().any(|g| g == "wl_subcompositor"),
        "wl_subcompositor is advertised (a stated debt — see this test's docs)"
    );
    assert_eq!(
        globals.len(),
        8,
        "no global is advertised that this test does not name: {globals:?}"
    );
}

/// **The inversion (M2 T7).** This test used to pin the wrong behaviour: a
/// subsurface's content accepted and silently dropped. It now asserts what a
/// client is entitled to — the content **composites**.
///
/// The comment is kept because the history is the point. T7b measured
/// `wl_subcompositor` as "advertised but unused" and proposed withdrawing it; T0
/// found that measurement was wrong (foot creates nine subsurfaces and fills
/// eight), that no refusal point could keep an honest client alive, and pinned the
/// silent wrongness here so it could not be forgotten. This is that debt
/// discharged: same test, opposite assertion.
#[test]
fn a_subsurface_composites_its_content() {
    let (scene, _host, mut client) = fixture();

    // A mapped toplevel to be the parent — a subsurface of an unmapped surface is
    // not mapped either, so the tree needs a root that is really on screen.
    let win = client.map_toplevel(32, 32, ShmFormat::Xrgb8888, &vec![255u8; 32 * 32 * 4]);
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.set_subsurface_position(&sub, 4, 4);
    client.draw(&child, 8, 8, &vec![128u8; 8 * 8 * 4]);
    // Synchronized by default: the child's content lands with the parent's commit.
    client.commit(&win.surface);
    client.roundtrip();

    let snapshot = scene.handle().snapshot();
    assert_eq!(
        snapshot.len(),
        2,
        "the window and its subsurface both composite"
    );
    // Composition order is bottom-to-top: parent first, then the child above it.
    assert_eq!(
        snapshot.nodes[1].size,
        (8, 8),
        "the child is the upper node"
    );
    assert_eq!(
        snapshot.nodes[1].transform,
        Transform::Translate { dx: 4, dy: 4 },
        "positioned relative to its parent, resolved to output coordinates"
    );
}

/// A **synchronized** subsurface's commit does not take effect until its parent
/// commits — the protocol's atomicity guarantee, and the reason a client can
/// update a window and its decorations without either being seen half-updated.
#[test]
fn a_sync_subsurfaces_commit_waits_for_its_parent() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let win = client.map_toplevel(32, 32, ShmFormat::Xrgb8888, &vec![255u8; 32 * 32 * 4]);
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.set_subsurface_position(&sub, 2, 2);
    client.roundtrip();

    // The child commits content. Nothing may appear yet.
    client.draw(&child, 8, 8, &vec![64u8; 8 * 8 * 4]);
    client.roundtrip();
    assert_eq!(
        h.snapshot().len(),
        1,
        "the child's commit is cached: only the window composites so far"
    );

    // The parent commits: now the child's state becomes current, atomically.
    client.commit(&win.surface);
    client.roundtrip();
    assert_eq!(
        h.snapshot().len(),
        2,
        "the parent's commit is what makes the child's content current"
    );
}

/// A **desynchronized** subsurface applies its own commits immediately — the
/// other half of the mode, and what a client uses for content that must not wait
/// on its parent (video, say).
#[test]
fn a_desync_subsurface_applies_its_own_commit() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let win = client.map_toplevel(32, 32, ShmFormat::Xrgb8888, &vec![255u8; 32 * 32 * 4]);
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.set_desync(&sub);
    // set_desync is itself double-buffered on the parent, so the parent commits
    // once to make the mode current.
    client.commit(&win.surface);
    client.roundtrip();

    client.draw(&child, 8, 8, &vec![64u8; 8 * 8 * 4]);
    client.roundtrip();
    assert_eq!(
        h.snapshot().len(),
        2,
        "a desync child does not wait for its parent"
    );
}

/// `place_below` puts a child **beneath** its parent — which is why the scene
/// stores the parent's own slot in its child order rather than assuming children
/// are always on top.
#[test]
fn place_below_puts_a_child_under_its_parent() {
    let (scene, _host, mut client) = fixture();

    let win = client.map_toplevel(32, 32, ShmFormat::Xrgb8888, &vec![255u8; 32 * 32 * 4]);
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.draw(&child, 8, 8, &vec![64u8; 8 * 8 * 4]);
    client.commit(&win.surface);
    client.roundtrip();

    let above = scene.handle().snapshot();
    assert_eq!(above.nodes[1].size, (8, 8), "by default the child is above");

    client.place_below(&sub, &win.surface);
    client.commit(&win.surface);
    client.roundtrip();

    let below = scene.handle().snapshot();
    assert_eq!(
        below.nodes[0].size,
        (8, 8),
        "after place_below the child composites first — beneath the window"
    );
    assert_eq!(below.nodes[1].size, (32, 32), "and the window is on top");
}

/// The mapping law through the tree: a subsurface of an **unmapped** parent is
/// not mapped either, and mapping the parent brings the whole tree with it.
#[test]
fn a_subsurface_is_mapped_only_while_its_parent_chain_is() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    // A parent that is a plain (roleless, unmapped) surface.
    let parent = client.create_surface();
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &parent);
    client.set_subsurface_position(&sub, 1, 1);
    client.draw(&child, 8, 8, &vec![64u8; 8 * 8 * 4]);
    client.commit(&parent);
    client.roundtrip();

    assert!(
        h.snapshot().is_empty(),
        "a child of an unmapped surface composites nothing, however much content \
         it has"
    );
}

/// The pixel-less subsurface — foot's case, the one nature provided. Role
/// assigned, position set, no buffer ever attached: it never composites and never
/// takes input, and it must not disturb anything that does.
#[test]
fn a_subsurface_without_content_never_composites() {
    let (scene, _host, mut client) = fixture();

    let win = client.map_toplevel(32, 32, ShmFormat::Xrgb8888, &vec![255u8; 32 * 32 * 4]);
    let child = client.create_surface();
    let sub = client.get_subsurface(&child, &win.surface);
    client.set_subsurface_position(&sub, 4, 4);
    client.commit(&child); // committed, but with no buffer — foot's border surface
    client.commit(&win.surface);
    client.roundtrip();

    let snapshot = scene.handle().snapshot();
    assert_eq!(
        snapshot.len(),
        1,
        "only the window composites; the empty subsurface contributes nothing"
    );
}

/// **A client-side-decorated window is placed by its declared geometry, not by
/// its surface origin.**
///
/// `xdg_surface.set_window_geometry` is how a CSD client says "my real window is
/// this rectangle; what lies outside it is title bar, border and shadow". foot
/// declares `(0, -26, 696, 494)` — its title bar sits 26 px *above* its surface
/// origin, in subsurfaces.
///
/// Placing the raw surface at the cascade origin therefore put the first window's
/// decorations off the top of the output, and only the first: every later cascade
/// slot has room above it. Roland found that by looking at the screen, which no
/// test of ours could have — every rig client draws at its own origin and declares
/// no geometry. This is that test, written after the fact and kept so the next
/// regression is caught by CI instead of by eye.
#[test]
fn a_window_is_placed_by_its_declared_geometry_not_its_surface_origin() {
    let (scene, _host, mut client) = fixture();

    // A window whose declared geometry begins 6 px above and 4 px left of its
    // surface origin — the shape of a client with decorations around it.
    let surface = client.create_surface();
    let xdg_surface = client.create_xdg_surface(&surface);
    let _toplevel = client.get_toplevel(&xdg_surface);
    client.commit(&surface);
    let serial = client.wait_for_configure();
    client.ack_configure(&xdg_surface, serial);
    client.set_window_geometry(&xdg_surface, -4, -6, 20, 20);
    client.draw(&surface, 20, 20, &vec![255u8; 20 * 20 * 4]);
    client.roundtrip();

    let placement = scene
        .handle()
        .query(|s| s.get(SurfaceId(0)).map(|n| n.transform));
    assert_eq!(
        placement,
        Some(Transform::Translate { dx: 4, dy: 6 }),
        "the surface is offset by the geometry's origin, so the *declared window* \
         lands at the cascade slot (0,0) — leaving the decoration overhang above \
         and left of it, on screen"
    );
}

/// `xdg_output` reports the output's **logical** geometry,/// `xdg_output` reports the output's **logical** geometry, and at scale 1 that is
/// its mode. Advertised alongside `wl_output`, so it is tested alongside it
/// rather than left as untested surface area.
#[test]
fn xdg_output_reports_logical_geometry() {
    let (_scene, _host, mut client) = fixture();
    let (w, h) = client.xdg_output_logical_size();
    assert_eq!(
        (w as u32, h as u32),
        DEFAULT_OUTPUT_SIZE,
        "logical size equals the mode at scale 1"
    );
    assert_eq!(
        client.xdg_output_logical_position(),
        (0, 0),
        "the single output sits at the origin"
    );
}
