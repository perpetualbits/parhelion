//! Protocol-rig integration tests: drive a real `ProtocolHost` with scripted
//! in-process clients and assert both sides — wire behaviour (globals seen,
//! round-trips complete) and canonical scene state (surface present after
//! create+commit; gone after destroy; cleaned up on disconnect; attributed to
//! the right client across two clients on one shard).
//!
//! Governing design: decision "2026-07-24 — Smithay threading fit",
//! `docs/smithay_threading_spike.md` §5.2, and `docs/scene_graph_v1.md` (the
//! scene owner these now publish to). These are the M0 ledger rig's tests,
//! **migrated to scene-state assertions** — the ledger was absorbed into the
//! scene (M1 T1), so the host now publishes into a `SceneThread` and the tests
//! query it. Fully deterministic: no external sockets or processes, and
//! synchronisation is on definite conditions (a client round-trip, or a scene
//! predicate), never a sleep.
//!
//! Determinism note: after `client.roundtrip()` returns, the dispatch thread has
//! already emitted its `ProtocolEvent`s into the scene channel (they are sent
//! during `dispatch_clients`, before the sync reply that unblocks the
//! round-trip). A subsequent `scene.query(...)` from the test thread therefore
//! observes them — FIFO on the scene channel, no sleep.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_core::protocol::{ProtocolHost, MAX_PENDING_FRAME_CALLBACKS};
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::{ClientKey, SceneThread, SurfaceId};
use parhelion_harness::protocol_rig::ScriptedClient;

/// Bring up a scene, a host publishing into it, and one scripted client over a
/// fresh socketpair. Returns all three; the client's `connect` already asserts
/// the compositor global was advertised (wire behaviour). Drop order (client →
/// host → scene, reverse of binding) tears down cleanly.
fn scene_host_client() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

/// create + commit → the surface is live in the scene and marked committed.
#[test]
fn create_and_commit_surface_appears_in_scene() {
    let (scene, _host, mut client) = scene_host_client();

    let surface = client.create_surface();
    client.commit(&surface);
    client.roundtrip();

    let h = scene.handle();
    assert_eq!(h.query(|s| s.surface_count()), 1, "one surface after create");

    // The one surface belongs to the one client and has committed.
    assert_eq!(h.query(|s| s.surface_count_for(ClientKey(0))), 1);
    let committed = h.query(|s| s.get(SurfaceId(0)).map(|n| n.committed));
    assert_eq!(committed, Some(true), "surface should be marked committed");
}

/// wl_surface.destroy → the surface leaves the scene.
#[test]
fn destroy_surface_removes_it_from_scene() {
    let (scene, _host, mut client) = scene_host_client();

    let surface = client.create_surface();
    client.commit(&surface);
    client.roundtrip();
    let h = scene.handle();
    assert_eq!(h.query(|s| s.surface_count()), 1);

    client.destroy(surface);
    client.roundtrip();
    assert_eq!(h.query(|s| s.surface_count()), 0, "surface gone after destroy");
}

/// Client disconnect (drop) → its surfaces are cleaned up. There is no
/// round-trip to sync on here, so we wait on the definite condition that the
/// scene has emptied (the dispatch thread emits `ClientGone` on EOF).
#[test]
fn client_disconnect_cleans_up_surfaces() {
    let (scene, _host, mut client) = scene_host_client();

    let surface = client.create_surface();
    client.commit(&surface);
    client.roundtrip();
    let h = scene.handle();
    assert_eq!(h.query(|s| s.surface_count()), 1);

    // Drop the client: closes the socket; the server observes EOF and runs
    // ClientData::disconnected, which emits ClientGone into the scene.
    drop(client);
    let emptied = h.wait_until(Duration::from_secs(5), |s| s.surface_count() == 0);
    assert!(emptied, "scene should empty after client disconnect");
}

/// Two clients on one shard: the scene attributes surfaces to the right client.
/// Client A (key 0) makes one surface; client B (key 1) makes two. If
/// attribution were confused the per-client counts would not be 1 and 2.
#[test]
fn two_clients_are_attributed_independently() {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    // Add both server ends first so client keys are assigned in a known order:
    // A → key 0, B → key 1 (the control channel is FIFO).
    let (sa, ca) = UnixStream::pair().expect("socketpair a");
    let (sb, cb) = UnixStream::pair().expect("socketpair b");
    host.add_client(sa);
    host.add_client(sb);
    let mut a = ScriptedClient::connect(ca);
    let mut b = ScriptedClient::connect(cb);

    // A: one surface.
    let sa1 = a.create_surface();
    a.commit(&sa1);
    a.roundtrip();

    // B: two surfaces.
    let sb1 = b.create_surface();
    let sb2 = b.create_surface();
    b.commit(&sb1);
    b.commit(&sb2);
    b.roundtrip();

    let h = scene.handle();
    assert_eq!(h.query(|s| s.surface_count()), 3, "1 + 2 surfaces total");
    assert_eq!(
        h.query(|s| s.surface_count_for(ClientKey(0))),
        1,
        "client A owns exactly one surface"
    );
    assert_eq!(
        h.query(|s| s.surface_count_for(ClientKey(1))),
        2,
        "client B owns exactly two surfaces"
    );
}

// ==========================================================================
// T2 — the reverse direction: frame callbacks, flush ownership, backpressure.
//
// These drive a real `RenderLoop` (with the headless CPU compositor) alongside
// the `ProtocolHost`, wired by `host.frame_presenter()`. A `render.tick(t)` is
// the render side deciding "frame presented at t"; the dispatch thread turns
// that into `wl_callback.done(t)` sends. Determinism: the callback is delivered
// on the socket no later than the round-trip's own sync reply, so pumping
// round-trips until it arrives (a definite condition) needs no sleep.
// ==========================================================================

/// Output size for the render loop in these tests — irrelevant to callbacks
/// (which fire regardless of what is composited), so kept tiny.
const RW: u32 = 4;
const RH: u32 = 4;
const CLEAR: [u8; 4] = [0, 0, 0, 255];

/// A scene + host + render loop wired together by the frame presenter, the
/// standard T2 fixture.
fn scene_host_render() -> (SceneThread, ProtocolHost, RenderLoop<CpuCompositor>) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let render = RenderLoop::new(scene.handle(), CpuCompositor::new(RW, RH, CLEAR))
        .with_presenter(host.frame_presenter());
    (scene, host, render)
}

/// Pump round-trips until the client has received at least `min` frame
/// callbacks, or a generous budget elapses. Deterministic: it waits on a
/// definite condition (the `done` event, which is already in flight), never a
/// timer. The budget is a safety net so a real regression fails loudly instead
/// of hanging.
fn pump_until_frame_dones(client: &mut ScriptedClient, min: usize) {
    for _ in 0..1000 {
        if client.frame_dones().len() >= min {
            return;
        }
        client.roundtrip();
    }
    panic!("frame callback not delivered within the round-trip budget");
}

/// The milestone's reverse-direction proof: an **attach-less** commit carrying a
/// frame request, then a render tick, delivers `wl_callback.done` with the
/// tick's deterministic timestamp. Attach-less on purpose — the callback fires
/// even though the surface is invisible, which is exactly why callback firing is
/// not gated on snapshot visibility in v1.
#[test]
fn scene_triggered_frame_callback_reaches_client() {
    let (_scene, host, mut render) = scene_host_render();
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let mut client = ScriptedClient::connect(client_end);

    let surface = client.create_surface();
    let _cb = client.frame(&surface); // frame request...
    client.commit(&surface); // ...carried by this attach-less commit
    client.roundtrip(); // the server now holds the pending callback

    // Render side: a frame was presented at a deterministic timestamp.
    render.tick(4242);

    pump_until_frame_dones(&mut client, 1);
    assert_eq!(
        client.frame_dones(),
        &[4242],
        "the scene-triggered callback carries the tick's timestamp"
    );
}

/// Callback lifecycle conformance: no `done` before the carrying commit; the
/// callback fires once its commit is presented, with the tick's timestamp; and
/// it is one-shot — a later tick with no new frame request delivers nothing.
#[test]
fn frame_callback_lifecycle_conformance() {
    let (_scene, host, mut render) = scene_host_render();
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let mut client = ScriptedClient::connect(client_end);

    let surface = client.create_surface();
    let _cb = client.frame(&surface); // frame request, NOT yet committed
    client.roundtrip(); // server holds it in *pending* (uncommitted)

    // A tick before the carrying commit must not fire the callback.
    render.tick(100);
    client.roundtrip();
    assert!(
        client.frame_dones().is_empty(),
        "no done before the carrying commit"
    );

    // Commit carries the callback into the eligible set; the next tick fires it.
    client.commit(&surface);
    client.roundtrip();
    render.tick(200);
    pump_until_frame_dones(&mut client, 1);
    assert_eq!(
        client.frame_dones(),
        &[200],
        "fires once its commit is presented, carrying the tick's timestamp"
    );

    // One-shot: a further tick with no new frame request delivers nothing more
    // (the callback was consumed/destroyed when it fired).
    render.tick(300);
    client.roundtrip();
    client.roundtrip();
    assert_eq!(
        client.frame_dones(),
        &[200],
        "callback is one-shot; not re-fired after done"
    );
}

/// Backpressure: a client flooding commits + frame requests is throttled (its
/// socket left unread) so its in-core queue stays bounded, while a well-behaved
/// second client is served in one tick and the flooder is never disconnected.
///
/// This test fails if the per-client throttle is removed: without it, client A's
/// buffered flood is admitted and the final backlog assertion (`<= bound + 1`)
/// blows past the bound. (Verified by deleting the skip in `pump_display`.)
#[test]
fn flooding_client_is_throttled_second_client_served_and_bounded() {
    let (scene, host, mut render) = scene_host_render();

    let (sa, ca) = UnixStream::pair().expect("socketpair a");
    let (sb, cb) = UnixStream::pair().expect("socketpair b");
    host.add_client(sa); // A → ClientKey(0)
    host.add_client(sb); // B → ClientKey(1)
    let mut a = ScriptedClient::connect(ca);
    let mut b = ScriptedClient::connect(cb);

    let surf_a = a.create_surface();

    // Flood A up to the bound: each frame+commit+roundtrip adds exactly one
    // pending callback. Stop the instant the bound is reached — one more
    // round-trip would hit A's now-throttled (unread) socket and block forever.
    let bound = MAX_PENDING_FRAME_CALLBACKS;
    while host.pending_frame_callbacks() < bound {
        let _cb = a.frame(&surf_a);
        a.commit(&surf_a);
        a.roundtrip();
    }
    assert_eq!(
        host.pending_frame_callbacks(),
        bound,
        "A reached exactly the bound"
    );

    // A keeps flooding, now WITHOUT round-trips: these pile into A's socket
    // (which the server has stopped reading) instead of blocking. They must not
    // be admitted while A is over the bound.
    const EXTRA: usize = 2000;
    for _ in 0..EXTRA {
        let _cb = a.frame(&surf_a);
        a.commit(&surf_a);
    }
    a.flush();

    // B behaves normally and is served, even as A floods.
    let surf_b = b.create_surface();
    let _cbb = b.frame(&surf_b);
    b.commit(&surf_b);
    b.roundtrip();
    // Extra passes give the dispatch loop every chance to (wrongly) drain A's
    // buffered flood — which it must not, because A is throttled.
    b.roundtrip();
    b.roundtrip();

    // Bounded memory: A's pending backlog stayed at the bound, its EXTRA flood
    // left unread. (+1 for B's single pending callback.)
    let pending = host.pending_frame_callbacks();
    assert!(
        pending <= bound + 1,
        "flood stayed bounded: {pending} pending (bound {bound}); throttle held A's socket unread"
    );

    // B's callback fires on the very next tick — one tick of latency, unaffected
    // by A's flood.
    render.tick(777);
    pump_until_frame_dones(&mut b, 1);
    assert_eq!(
        b.frame_dones(),
        &[777],
        "B served in one tick despite A flooding"
    );

    // A is throttled, not killed: still connected, its surface still in the scene.
    let h = scene.handle();
    assert_eq!(
        h.query(|s| s.surface_count_for(ClientKey(0))),
        1,
        "A still connected (throttled, not disconnected)"
    );
}
