//! `parhelion-dev` — the M1 interactive artifact: a Parhelion compositor in a
//! desktop window, with a Wayland socket real clients can connect to.
//!
//! Governing design: `docs/plans/m1_tasks.md` T6/T7. Deliberately **thin**: it
//! wires together the scene, the protocol host, the render loop, and the nested
//! backend, and does nothing else. Every piece of logic lives in the library
//! crates, where it is testable without a display.
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
//!
//! # `--headless`
//!
//! ```text
//! parhelion-dev --headless [--socket PATH]
//! ```
//!
//! Serves the socket and runs the render loop with **no window**, which is what
//! makes the binary's own plumbing — socket, shutdown, cleanup — testable on a
//! machine with no display (`crates/harness/tests/dev_binary.rs`). It is not a
//! second compositor: same scene, same host, same render loop, minus the winit
//! parts.
//!
//! # Shutdown
//!
//! SIGINT (Ctrl-C) and SIGTERM end the loop through its normal path, so the
//! listening socket and its `.lock` are unlinked on the way out. `SIGKILL`
//! cannot be caught and will leave them behind; wayland-server's lock protocol
//! makes that harmless on the next bind.

use std::time::{Duration, Instant};

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_winit::shutdown::ShutdownFlag;
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

/// How often the headless loop produces a frame. Frames are what deliver frame
/// callbacks, and clients throttle their drawing on those — so a headless run
/// must keep ticking or every client freezes after its first frame. ~60 Hz.
const HEADLESS_TICK: Duration = Duration::from_millis(16);

fn main() {
    // --headless: serve without a window (tests, and machines with no display).
    // --socket PATH: bind an explicit path instead of $XDG_RUNTIME_DIR/wayland-N.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let headless = args.iter().any(|a| a == "--headless");
    let socket_path = args
        .iter()
        .position(|a| a == "--socket")
        .and_then(|i| args.get(i + 1))
        .cloned();

    // Signal handlers first: a compositor that will not shut down cleanly is
    // worth knowing about before it binds anything.
    let shutdown = ShutdownFlag::new();
    if let Err(e) = shutdown.install_signal_handlers() {
        eprintln!("parhelion-dev: could not install signal handlers ({e}); shutdown may litter");
    }

    // Canonical state, protocol frontend, render loop — the same three pieces
    // every test builds, wired the same way.
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let mut render = RenderLoop::new(scene.handle(), CpuCompositor::new(WIDTH, HEIGHT, CLEAR))
        .with_presenter(host.frame_presenter());
    host.set_output_size(WIDTH, HEIGHT);

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

    // The Wayland socket. Failing to bind is not fatal in windowed mode (the
    // window is still worth showing) but is fatal headless, where the socket is
    // the entire point.
    let bound = match &socket_path {
        Some(path) => host.listen_at(path.as_str()).map(|()| path.clone()),
        None => host.listen_auto(),
    };
    match bound {
        Ok(name) => {
            println!("parhelion-dev: WAYLAND_DISPLAY={name}");
            println!("parhelion-dev: try  WAYLAND_DISPLAY={name} foot");
        }
        Err(e) => {
            eprintln!("parhelion-dev: no Wayland socket ({e})");
            if headless {
                std::process::exit(1);
            }
        }
    }

    if headless {
        run_headless(&mut render, &shutdown);
        // Dropping `host` here stops the dispatch thread, which drops the
        // listening socket, which unlinks the socket and its lock file.
        drop(host);
        return;
    }

    let event_loop = EventLoop::new().expect("create the winit event loop");
    // Poll rather than Wait: the M1 render loop has no frame scheduler (that
    // arrives with the DRM backend in M2), so frames are produced as fast as the
    // loop turns. Honest for a development backend, wrong for a real one — and
    // said so here rather than discovered later.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut backend = NestedBackend::new(
        scene.handle(),
        host,
        render,
        "Parhelion (nested)",
        shutdown,
    );
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

/// The windowless loop: tick the render loop at a steady rate until a signal
/// arrives. The ticks are what fire frame callbacks, so clients keep drawing.
fn run_headless(render: &mut RenderLoop<CpuCompositor>, shutdown: &ShutdownFlag) {
    println!("parhelion-dev: headless (no window); Ctrl-C or SIGTERM to stop");
    let start = Instant::now();
    while !shutdown.is_raised() {
        render.tick(start.elapsed().as_millis() as u32);
        std::thread::sleep(HEADLESS_TICK);
    }
    println!("parhelion-dev: shutting down");
}
