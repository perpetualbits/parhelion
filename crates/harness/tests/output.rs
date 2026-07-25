//! T7 `wl_output` tests: the screen a client asks about before it draws.
//!
//! Governing design: `docs/scene_graph_v1.md` §12. `wl_output` was
//! pre-authorized for this task because a real client realistically requires one
//! — it needs the size, scale, and refresh before it can lay itself out — and it
//! is therefore implemented properly rather than stubbed. These tests are what
//! "properly" means in practice: real values, a `done` that closes the batch, and
//! `wl_surface.enter`/`leave` following map and unmap.
//!
//! Determinism: every assertion follows a client round-trip, so the events are
//! already in flight; no sleeps.

use std::os::unix::net::UnixStream;

use parhelion_core::protocol::{ProtocolHost, DEFAULT_OUTPUT_SIZE, OUTPUT_REFRESH_MHZ};
use parhelion_core::scene::SceneThread;
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

const W: i32 = 16;
const H: i32 = 16;

fn white() -> Vec<u8> {
    vec![255u8; (W * H * 4) as usize]
}

fn fixture() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

/// A client that binds the output learns a real mode, scale 1, and gets a `done`
/// closing the batch — the three facts it needs before drawing a single pixel.
#[test]
fn output_advertises_a_real_mode_scale_and_done() {
    let (_scene, _host, mut client) = fixture();
    client.roundtrip();
    client.roundtrip();

    let (w, h, refresh) = client.output_mode().expect("mode advertised");
    assert_eq!(
        (w as u32, h as u32),
        DEFAULT_OUTPUT_SIZE,
        "the mode is the compositor's actual output size, not a placeholder 0×0"
    );
    assert_eq!(
        refresh, OUTPUT_REFRESH_MHZ,
        "refresh is advertised in millihertz"
    );
    assert_eq!(client.output_scale(), 1, "scale 1 — we implement no scaling yet");
    assert!(
        client.output_done_count() >= 1,
        "a done event closed the atomic batch of output state"
    );
}

/// The backend telling the compositor its window resized re-advertises the mode,
/// so a client that laid itself out for the old size learns the new one.
#[test]
fn resizing_the_output_readvertises_the_mode() {
    let (_scene, host, mut client) = fixture();
    client.roundtrip();
    client.roundtrip();
    let before = client.output_done_count();

    host.set_output_size(800, 600);
    for _ in 0..1000 {
        if client.output_done_count() > before {
            break;
        }
        client.roundtrip();
    }

    assert_eq!(
        client.output_mode().map(|(w, h, _)| (w, h)),
        Some((800, 600)),
        "the new size reached the client"
    );
    assert!(
        client.output_done_count() > before,
        "and it arrived as a batch closed by its own done"
    );
}

/// A mapped window is on the output and is told so; unmapping takes it off
/// again. This is what lets a client know which screen it is showing on (and, in
/// M2, which scale to render for).
#[test]
fn surfaces_enter_and_leave_the_output_with_map_and_unmap() {
    let (_scene, _host, mut client) = fixture();

    let mut win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white());
    let id = client.surface_id(&win.surface);
    client.roundtrip();
    assert_eq!(
        client.surface_outputs(),
        &[(id, true)],
        "the mapped window entered the output"
    );

    // A second content commit must not re-announce it.
    win.pool.write(&white());
    client.attach(&win.surface, &win.buffer);
    client.commit(&win.surface);
    client.roundtrip();
    assert_eq!(
        client.surface_outputs(),
        &[(id, true)],
        "entering is idempotent — no duplicate enter on a redraw"
    );

    // Unmap → leave.
    client.attach_null(&win.surface);
    client.commit(&win.surface);
    client.roundtrip();
    client.roundtrip();
    assert_eq!(
        client.surface_outputs(),
        &[(id, true), (id, false)],
        "unmapping the window left the output"
    );
}
