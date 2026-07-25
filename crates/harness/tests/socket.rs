//! T6 listening-socket test: the plumbing the dev binary is made of, exercised
//! without a display.
//!
//! Governing design: `docs/scene_graph_v1.md` §11.4. `parhelion-dev` is two
//! things bolted together — a winit window and a Wayland socket — and only the
//! window needs a display. So the socket half lives in the core
//! (`ProtocolHost::listen_at` / `listen_auto`) and is tested here against a real
//! client over a real Unix socket, which is what makes "the binary serves
//! clients" a checked claim rather than a hopeful one.
//!
//! The socket is bound inside the test's own temporary directory, so this
//! depends on no `$XDG_RUNTIME_DIR` and cannot collide with a running session.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use parhelion_core::protocol::ProtocolHost;
use parhelion_core::scene::{SceneThread, SurfaceId};
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

/// A client that connects over a listening socket is admitted through the same
/// seam as the rig's socketpair clients, and everything downstream works: it
/// binds the globals, maps a toplevel, and the window reaches the scene.
#[test]
fn a_client_connecting_over_the_listening_socket_is_served() {
    let dir = tempfile::tempdir().expect("temp dir for the socket");
    let path = dir.path().join("wayland-test");

    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    host.listen_at(&path).expect("bind the listening socket");

    // Connect as an external client would. The socket is registered with the
    // dispatch loop asynchronously, so retry briefly on ENOENT/ECONNREFUSED —
    // a definite condition (the file exists and accepts) with a loud budget.
    let stream = connect_with_budget(&path);
    let mut client = ScriptedClient::connect(stream);

    let _win = client.map_toplevel(16, 16, ShmFormat::Xrgb8888, &[255u8; 16 * 16 * 4]);

    let h = scene.handle();
    assert_eq!(
        h.query(|s| s.surface_count()),
        1,
        "the socket-connected client's surface reached the scene"
    );
    assert_eq!(
        h.query(|s| s.get(SurfaceId(0)).map(|n| n.is_visible())),
        Some(true),
        "and its toplevel mapped — the whole path works over a real socket"
    );
}

/// Two clients over the same socket are admitted independently — the accept loop
/// keeps serving after the first connection, which is the failure a single-shot
/// accept would hide.
#[test]
fn the_socket_keeps_accepting_after_the_first_client() {
    let dir = tempfile::tempdir().expect("temp dir for the socket");
    let path = dir.path().join("wayland-test");

    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    host.listen_at(&path).expect("bind the listening socket");

    let mut a = ScriptedClient::connect(connect_with_budget(&path));
    let _wa = a.map_toplevel(16, 16, ShmFormat::Xrgb8888, &[255u8; 16 * 16 * 4]);
    let mut b = ScriptedClient::connect(connect_with_budget(&path));
    let _wb = b.map_toplevel(16, 16, ShmFormat::Xrgb8888, &[255u8; 16 * 16 * 4]);

    let h = scene.handle();
    assert_eq!(
        h.query(|s| s.surface_count()),
        2,
        "both clients were admitted and both windows reached the scene"
    );
}

/// Connect to `path`, retrying while the dispatch thread is still registering the
/// socket. Bounded and loud: a real failure panics with the last error rather
/// than hanging.
fn connect_with_budget(path: &std::path::Path) -> UnixStream {
    let mut last = None;
    for _ in 0..500 {
        match UnixStream::connect(path) {
            Ok(stream) => return stream,
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    panic!("could not connect to {path:?}: {last:?}");
}
