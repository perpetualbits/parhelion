//! Protocol-rig integration tests: drive a real `ProtocolHost` with scripted
//! in-process clients and assert both sides — wire behaviour (globals seen,
//! round-trips complete) and scene-ledger state (surface present after
//! create+commit; gone after destroy; cleaned up on disconnect; attributed to
//! the right client across two clients on one shard).
//!
//! Governing design: decision "2026-07-24 — Smithay threading fit" and
//! `docs/smithay_threading_spike.md` §5.2. These are the four "first tests" the
//! task calls for. Fully deterministic: no external sockets or processes, and
//! synchronisation is on definite conditions (a client round-trip, or a ledger
//! predicate), never a sleep.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use parhelion_core::ledger::ClientKey;
use parhelion_core::protocol::ProtocolHost;
use parhelion_harness::protocol_rig::ScriptedClient;

/// Bring up a host and connect one scripted client over a fresh socketpair.
/// Returns both; the client's `connect` already asserts the compositor global
/// was advertised (wire behaviour).
fn host_with_client() -> (ProtocolHost, ScriptedClient) {
    let host = ProtocolHost::new();
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (host, client)
}

/// create + commit → the surface is live in the ledger and marked committed.
#[test]
fn create_and_commit_surface_appears_in_ledger() {
    let (mut host, mut client) = host_with_client();

    let surface = client.create_surface();
    client.commit(&surface);
    client.roundtrip();

    host.sync();
    assert_eq!(host.ledger().surface_count(), 1, "one surface after create");

    // The one surface belongs to the one client and has committed.
    assert_eq!(host.ledger().surface_count_for(ClientKey(0)), 1);
    let rec = host
        .ledger()
        .get(parhelion_core::ledger::SurfaceId(0))
        .expect("surface 0 present");
    assert!(rec.committed, "surface should be marked committed");
}

/// wl_surface.destroy → the surface leaves the ledger.
#[test]
fn destroy_surface_removes_it_from_ledger() {
    let (mut host, mut client) = host_with_client();

    let surface = client.create_surface();
    client.commit(&surface);
    client.roundtrip();
    host.sync();
    assert_eq!(host.ledger().surface_count(), 1);

    client.destroy(surface);
    client.roundtrip();
    host.sync();
    assert_eq!(host.ledger().surface_count(), 0, "surface gone after destroy");
}

/// Client disconnect (drop) → its surfaces are cleaned up. There is no
/// round-trip to sync on here, so we wait on the definite condition that the
/// ledger has emptied.
#[test]
fn client_disconnect_cleans_up_surfaces() {
    let (mut host, mut client) = host_with_client();

    let surface = client.create_surface();
    client.commit(&surface);
    client.roundtrip();
    host.sync();
    assert_eq!(host.ledger().surface_count(), 1);

    // Drop the client: closes the socket; the server observes EOF and runs
    // ClientData::disconnected, which publishes ClientGone.
    drop(client);
    let emptied = host.wait_until(Duration::from_secs(5), |ledger| ledger.surface_count() == 0);
    assert!(emptied, "ledger should empty after client disconnect");
}

/// Two clients on one shard: the ledger attributes surfaces to the right
/// client. Client A (key 0) makes one surface; client B (key 1) makes two. If
/// attribution were confused the per-client counts would not be 1 and 2.
#[test]
fn two_clients_are_attributed_independently() {
    let host_and_clients = {
        let host = ProtocolHost::new();
        // Add both server ends first so client keys are assigned in a known
        // order: A → key 0, B → key 1 (the control channel is FIFO).
        let (sa, ca) = UnixStream::pair().expect("socketpair a");
        let (sb, cb) = UnixStream::pair().expect("socketpair b");
        host.add_client(sa);
        host.add_client(sb);
        let a = ScriptedClient::connect(ca);
        let b = ScriptedClient::connect(cb);
        (host, a, b)
    };
    let (mut host, mut a, mut b) = host_and_clients;

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

    host.sync();
    assert_eq!(host.ledger().surface_count(), 3, "1 + 2 surfaces total");
    assert_eq!(
        host.ledger().surface_count_for(ClientKey(0)),
        1,
        "client A owns exactly one surface"
    );
    assert_eq!(
        host.ledger().surface_count_for(ClientKey(1)),
        2,
        "client B owns exactly two surfaces"
    );
}
