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

use parhelion_core::protocol::ProtocolHost;
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
