//! `parhelion-dev` — the M1 interactive artifact: a Parhelion compositor in a
//! desktop window, with a Wayland socket real clients can connect to.
//!
//! Governing design: `docs/plans/m1_tasks.md` T6 (and T7, which points `foot` at
//! this). Deliberately **thin**: it wires together the scene, the protocol host,
//! the render loop, and the nested backend, and does nothing else. Every piece of
//! logic lives in the library crates, where it is testable without a display —
//! the socket half of this binary has its own headless test
//! (`crates/harness/tests/socket.rs`), which is why "it serves clients" is a
//! checked claim.
//!
//! # Running it
//!
//! ```text
//! cargo run -p parhelion-backend-winit --bin parhelion-dev
//! ```
//!
//! It prints the display name it bound; point a client at it with
//! `WAYLAND_DISPLAY=<name> foot`. A grey placeholder rectangle is drawn by the
//! core itself (a C10-style scene-injected node) so the window is obviously
//! alive before any client connects.

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_winit::NestedBackend;
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::{ClientKey, SceneThread, SurfaceId, Transform};
use winit::event_loop::{ControlFlow, EventLoop};

/// Initial window size. Not a golden, not a constraint — just a comfortable
/// default; the window is resizable and the output follows it.
const WIDTH: u32 = 960;
const HEIGHT: u32 = 640;

/// The desktop background colour the compositor clears to.
const CLEAR: [u8; 4] = [24, 26, 32, 255];

/// A placeholder panel drawn by the core, so an empty desktop still looks alive.
const PLACEHOLDER: [u8; 4] = [64, 72, 96, 255];
const PLACEHOLDER_SID: SurfaceId = SurfaceId(10_000);
const PLACEHOLDER_CLIENT: ClientKey = ClientKey(10_000);

fn main() {
    // Canonical state, protocol frontend, render loop — the same three pieces
    // every test builds, wired the same way.
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let render = RenderLoop::new(scene.handle(), CpuCompositor::new(WIDTH, HEIGHT, CLEAR))
        .with_presenter(host.frame_presenter());

    // A core-injected node: no client, no protocol, just something on screen.
    // This is the shape a C10 fallback surface takes (`NodeRole::CoreOwned`).
    scene.handle().place_solid(
        PLACEHOLDER_SID,
        PLACEHOLDER_CLIENT,
        Transform::Translate { dx: 40, dy: 40 },
        (240, 160),
        0,
        PLACEHOLDER,
        true,
    );

    // The Wayland socket. Failing to bind is not fatal: the window is still worth
    // showing, and the reason is worth printing rather than swallowing.
    match host.listen_auto() {
        Ok(name) => {
            println!("parhelion-dev: WAYLAND_DISPLAY={name}");
            println!("parhelion-dev: try  WAYLAND_DISPLAY={name} foot");
        }
        Err(e) => eprintln!("parhelion-dev: no Wayland socket ({e}); window only"),
    }

    let event_loop = EventLoop::new().expect("create the winit event loop");
    // Poll rather than Wait: the M1 render loop has no frame scheduler (that
    // arrives with the DRM backend in M2), so frames are produced as fast as the
    // loop turns. Honest for a development backend, wrong for a real one — and
    // said so here rather than discovered later.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut backend = NestedBackend::new(scene.handle(), host, render, "Parhelion (nested)");
    event_loop
        .run_app(&mut backend)
        .expect("run the winit event loop");

    // Exit-time honesty: if keys were dropped for want of an evdev mapping, say
    // so, with the number. A silent drop is indistinguishable from a broken
    // keyboard path.
    let dropped = backend.dropped_keys();
    if dropped > 0 {
        eprintln!("parhelion-dev: {dropped} key event(s) had no evdev mapping and were dropped");
    }
}
