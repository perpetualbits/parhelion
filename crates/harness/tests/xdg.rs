//! T5 xdg-shell tests: the toplevel lifecycle, the protocol errors that guard
//! it, and the mapping semantics it installs.
//!
//! Governing design: `docs/scene_graph_v1.md` §10 (mapping semantics and roles),
//! `docs/plans/m1_tasks.md` T5, and `CORE-BOUNDARY.md` C10 (the placement
//! fallback these assert the determinism of).
//!
//! Two things these tests are strict about:
//!
//! 1. **Errors are asserted by code**, never by "the client died". A compositor
//!    that disconnects a client for the wrong reason is still broken, and only
//!    the code distinguishes the two.
//! 2. **Nothing sleeps.** Synchronisation is on definite conditions: a client
//!    round-trip (the server has processed everything sent before it), a
//!    configure the compositor owes, or a scene predicate.

use std::os::unix::net::UnixStream;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_core::protocol::{
    ProtocolHost, CASCADE_ORIGIN_X, CASCADE_ORIGIN_Y, CASCADE_STEP_X, CASCADE_STEP_Y,
};
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::{SceneThread, SurfaceId, Transform};
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

/// Output size for the render loops here — large enough to hold two cascaded
/// windows, small enough to stay cheap.
const RW: u32 = 96;
const RH: u32 = 64;
const CLEAR: [u8; 4] = [0, 0, 0, 255];

/// Window size used by most tests.
const W: i32 = 16;
const H: i32 = 16;

/// Opaque white `wl_shm` bytes (`xrgb8888`: `[B, G, R, X]`) for a `w×h` buffer.
fn white(w: i32, h: i32) -> Vec<u8> {
    vec![255u8; (w * h * 4) as usize]
}

/// The standard fixture: a scene, a host publishing into it, and one connected
/// scripted client.
fn fixture() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

// ==========================================================================
// The lifecycle.
// ==========================================================================

/// The happy path, step by step: the initial (buffer-less) commit is answered
/// with exactly one configure suggesting 0×0 ("you choose"); after acking it the
/// client may commit a buffer, and *that* commit maps the window.
#[test]
fn configure_ack_dance_maps_the_toplevel() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let surface = client.create_surface();
    let xdg_surface = client.create_xdg_surface(&surface);
    let _toplevel = client.get_toplevel(&xdg_surface);

    // Role assigned, nothing committed: the node exists and is invisible.
    client.roundtrip();
    assert_eq!(h.query(|s| s.surface_count()), 1, "the node is live");
    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.is_visible())),
        Some(false),
        "a toplevel with no content is not mapped"
    );

    // The initial commit: no buffer. The compositor owes a configure.
    client.commit(&surface);
    let serial = client.wait_for_configure();
    assert_eq!(
        client.toplevel_configures(),
        &[(0, 0)],
        "one configure, size 0×0 — the client picks its own size"
    );

    // Ack, then draw + attach + commit: this commit maps the window.
    client.ack_configure(&xdg_surface, serial);
    let mut pool = client.create_pool((W * H * 4) as usize);
    pool.write(&white(W, H));
    let buffer = client.create_buffer(&pool, W, H, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.is_visible())),
        Some(true),
        "mapped after the acked buffer commit"
    );
    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.size)),
        Some((W as u32, H as u32)),
        "the buffer defines the node's size"
    );
    assert_eq!(
        client.toplevel_configures().len(),
        1,
        "no further configure was sent (nothing changed)"
    );
}

/// The migration's headline, from the wire side: a `wl_surface` that commits a
/// perfectly good buffer but never takes a role is **never displayed**. Before
/// T5 this composited; per Wayland it must not.
#[test]
fn roleless_surface_is_never_displayed() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let surface = client.create_surface();
    let mut pool = client.create_pool((W * H * 4) as usize);
    pool.write(&white(W, H));
    let buffer = client.create_buffer(&pool, W, H, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    assert_eq!(
        h.query(|s| s.surface_count()),
        1,
        "the surface is live in the scene"
    );
    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.is_visible())),
        Some(false),
        "but a surface with no role contributes no pixels"
    );
    assert!(
        h.snapshot().is_empty(),
        "and so appears in no snapshot — nothing to composite"
    );
}

/// Title and app_id reach canonical state (I-5), and are the only things the
/// role carries in M1 — no behaviour hangs off them.
#[test]
fn title_and_app_id_reach_the_scene() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    client.set_title(&win.toplevel, "parhelion window");
    client.set_app_id(&win.toplevel, "org.parhelion.Test");
    client.roundtrip();

    let role = h.query(|s| s.get(SurfaceId(0)).map(|n| n.role.clone()));
    let role = role.expect("node present");
    let toplevel = role.toplevel().expect("node carries the toplevel role");
    assert_eq!(toplevel.title.as_deref(), Some("parhelion window"));
    assert_eq!(toplevel.app_id.as_deref(), Some("org.parhelion.Test"));
}

/// `xdg_wm_base` ping/pong: the compositor pings, the client pongs, and the
/// core sees the answer. (M1 has no ping *scheduler* — this is the mechanism.)
#[test]
fn ping_is_answered_with_pong() {
    let (_scene, host, mut client) = fixture();
    let _win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));

    host.ping_clients();

    // Pump until the ping arrives (it crosses the control channel and the socket
    // asynchronously), then until the pong has come back. Both are definite
    // conditions with a loud budget, not sleeps.
    for _ in 0..1000 {
        if client.pings_received() > 0 {
            break;
        }
        client.roundtrip();
    }
    assert_eq!(client.pings_received(), 1, "client received the ping");

    for _ in 0..1000 {
        if host.pongs_received() > 0 {
            break;
        }
        client.roundtrip();
    }
    assert_eq!(host.pongs_received(), 1, "the core saw the pong");
}

// ==========================================================================
// Protocol errors. Each asserts the specific error, not merely a disconnect.
// ==========================================================================

/// Committing a buffer before acking the initial configure is a protocol error,
/// and maps nothing.
///
/// The code is `xdg_surface.not_constructed` (1): Smithay's `ensure_configured`
/// posts that where the spec's dedicated code is `unconfigured_buffer` (3), and
/// the `xdg_surface` object is not reachable through Smithay's public API for us
/// to post our own. The deviation is recorded in `docs/scene_graph_v1.md` §10 —
/// this test pins whichever error we actually send, which is the point.
#[test]
fn buffer_before_ack_is_a_protocol_error() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let surface = client.create_surface();
    let xdg_surface = client.create_xdg_surface(&surface);
    let _toplevel = client.get_toplevel(&xdg_surface);
    client.commit(&surface); // initial commit → compositor sends a configure
    let _serial = client.wait_for_configure(); // received, deliberately NOT acked

    let mut pool = client.create_pool((W * H * 4) as usize);
    pool.write(&white(W, H));
    let buffer = client.create_buffer(&pool, W, H, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);

    let err = client.expect_protocol_error();
    assert_eq!(err.interface, "xdg_surface", "error posted on the xdg_surface");
    assert_eq!(
        err.code, 1,
        "xdg_surface.error.not_constructed — {}",
        err.message
    );

    // The offending commit mapped nothing (the client is being dropped, so wait
    // on the definite condition that its surfaces are gone).
    assert!(
        h.wait_until(std::time::Duration::from_secs(5), |s| s.surface_count() == 0),
        "the erroring client's surfaces are cleaned up, and none ever mapped"
    );
}

/// Acking a serial the compositor never sent is a protocol error.
#[test]
fn bad_ack_serial_is_a_protocol_error() {
    let (_scene, _host, mut client) = fixture();

    let surface = client.create_surface();
    let xdg_surface = client.create_xdg_surface(&surface);
    let _toplevel = client.get_toplevel(&xdg_surface);
    client.commit(&surface);
    let serial = client.wait_for_configure();

    // A serial far from anything issued.
    client.ack_configure(&xdg_surface, serial.wrapping_add(9_999));

    let err = client.expect_protocol_error();
    assert_eq!(err.interface, "xdg_wm_base", "error posted on the wm_base");
    assert_eq!(
        err.code, 4,
        "xdg_wm_base.error.invalid_surface_state — {}",
        err.message
    );
}

/// A surface may hold only one role: asking a surface that is already a toplevel
/// for the popup role is a protocol error.
///
/// The provocation is deliberately a *different* role. Asking twice for the
/// **same** role is idempotent in Smithay's role bookkeeping (`set_role` only
/// rejects a conflicting role), so a repeated `get_toplevel` would prove nothing
/// about the rule — it would only pin an implementation detail.
#[test]
fn second_role_on_a_surface_is_a_protocol_error() {
    let (_scene, _host, mut client) = fixture();

    // One wl_surface, two xdg_surfaces: the first takes the toplevel role...
    let surface = client.create_surface();
    let first = client.create_xdg_surface(&surface);
    let _toplevel = client.get_toplevel(&first);
    client.roundtrip();

    // ...and the second asks the same wl_surface for the popup role.
    let second = client.create_xdg_surface(&surface);
    let positioner = client.create_positioner();
    let _popup = client.get_popup(&second, &first, &positioner);

    let err = client.expect_protocol_error();
    assert_eq!(err.interface, "xdg_wm_base", "error posted on the wm_base");
    assert_eq!(err.code, 0, "xdg_wm_base.error.role — {}", err.message);
}

// ==========================================================================
// Unmap, and the damage it must raise.
// ==========================================================================

/// Unmap by `xdg_toplevel.destroy`: the node loses both its content and its
/// role, stops being displayed, and the pixels it occupied are damaged — the
/// structural-damage obligation, read off the render counters.
#[test]
fn destroying_the_toplevel_unmaps_with_damage() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let mut render = RenderLoop::new(h.clone(), CpuCompositor::new(RW, RH, CLEAR));
    render.tick(0); // drains the mapping damage
    let redrawn_after_map = render.counters().pixels_redrawn;

    win.toplevel.destroy();
    client.roundtrip();

    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.is_visible())),
        Some(false),
        "destroying the role object unmaps the window"
    );
    assert_eq!(
        h.query(|s| s.surface_count()),
        1,
        "the wl_surface outlives its role object, so the node stays live"
    );

    render.tick(0);
    let delta = render.counters().pixels_redrawn - redrawn_after_map;
    assert!(
        delta >= (W * H) as u64,
        "unmap damaged at least the window's extent ({delta} < {})",
        W * H
    );
}

/// Unmap by null attach, and the protocol's consequence: an unmapped toplevel
/// must run the initial commit/configure sequence again before it may map. The
/// re-map is driven through the full dance and must succeed.
#[test]
fn null_attach_unmaps_and_remap_needs_the_dance_again() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let mut win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let mut render = RenderLoop::new(h.clone(), CpuCompositor::new(RW, RH, CLEAR));
    render.tick(0);
    let redrawn_after_map = render.counters().pixels_redrawn;

    // Unmap.
    client.attach_null(&win.surface);
    client.commit(&win.surface);
    client.roundtrip();
    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.is_visible())),
        Some(false),
        "null attach unmaps"
    );
    render.tick(0);
    let delta = render.counters().pixels_redrawn - redrawn_after_map;
    assert!(
        delta >= (W * H) as u64,
        "unmap damaged at least the window's extent ({delta} < {})",
        W * H
    );

    // Re-map: the dance starts over — a buffer-less commit earns a fresh
    // configure, which must be acked before content is accepted again.
    client.commit(&win.surface);
    let serial = client.wait_for_configure();
    assert_eq!(
        client.toplevel_configures().len(),
        2,
        "a second configure was sent for the re-map"
    );
    client.ack_configure(&win.xdg_surface, serial);
    win.pool.write(&white(W, H));
    client.attach(&win.surface, &win.buffer);
    client.commit(&win.surface);
    client.roundtrip();

    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.is_visible())),
        Some(true),
        "the window maps again after re-running the dance"
    );
}

// ==========================================================================
// C10 fallback placement.
// ==========================================================================

/// Two toplevels cascade deterministically: the first at the cascade origin, the
/// second one step along both axes. This is C10's temporary placement — the
/// property under test is *determinism*, which the goldens depend on (the
/// pixel-level version of this lives in `xdg_render.rs`).
#[test]
fn two_toplevels_cascade_deterministically() {
    let (scene, _host, mut client) = fixture();
    let h = scene.handle();

    let _a = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let _b = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));

    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.transform)),
        Some(Transform::Translate {
            dx: CASCADE_ORIGIN_X,
            dy: CASCADE_ORIGIN_Y
        }),
        "first toplevel sits at the cascade origin"
    );
    assert_eq!(
        h.query(|s| s.get(SurfaceId(1)).map(|n| n.transform)),
        Some(Transform::Translate {
            dx: CASCADE_ORIGIN_X + CASCADE_STEP_X,
            dy: CASCADE_ORIGIN_Y + CASCADE_STEP_Y
        }),
        "second toplevel is offset by exactly one cascade step"
    );
}
