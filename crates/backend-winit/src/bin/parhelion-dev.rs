//! `parhelion-dev` — the interactive artifact: a Parhelion compositor with a
//! Wayland socket real clients can connect to, in a desktop window (M1 T6/T7) or
//! on real hardware from a TTY (M2 T1).
//!
//! Governing design: `docs/plans/m1_tasks.md` T6/T7, `docs/plans/m2_tasks.md` T1.
//! Deliberately **thin**: it wires together the scene, the protocol host, and a
//! backend, and does nothing else. Every piece of logic lives in the library
//! crates, where it is testable without a display and without a GPU.
//!
//! # Backends
//!
//! ```text
//! parhelion-dev                      # nested: a window on your desktop (default)
//! parhelion-dev --headless           # no window at all (tests, headless machines)
//! parhelion-dev --drm                # real hardware, from a TTY (M2 T1)
//! ```
//!
//! Common flags: `--socket PATH` binds an explicit socket path instead of
//! `$XDG_RUNTIME_DIR/wayland-N`; `--exit-after=SECONDS` shuts the compositor down
//! on a timer.
//!
//! # `--drm`, and reading this before your first TTY run
//!
//! `--drm` takes over the console: DRM master, the whole screen, and the mode.
//! Two things are **not** implemented yet and their absence is expected, not a
//! bug:
//!
//! - **there is no input on metal** — libinput and the real T-input thread are
//!   M2 T2, so the keyboard is silent and nothing responds to typing;
//! - **there is no cursor** — the hardware cursor plane is also T2.
//!
//! So a first run should use `--exit-after=20`: the compositor puts pixels on the
//! screen, holds them for twenty seconds, and gives the console back on its own.
//! `--drm-device PATH` picks a specific card on a machine with more than one;
//! without it the first `/dev/dri/card*` with something plugged in wins.
//!
//! The escape hatches, written down before they are needed: switching VT
//! (Ctrl-Alt-F2 …) suspends the compositor and returns you to a normal console;
//! `ssh` from another machine reaches a shell that can `kill` it; and SIGTERM
//! ends it cleanly from anywhere.
//!
//! # Shutdown
//!
//! SIGINT (Ctrl-C) and SIGTERM end the loop through its normal path, so the
//! listening socket and its `.lock` are unlinked on the way out, and the DRM
//! device is released so the kernel restores the console. `SIGKILL` cannot be
//! caught and will leave the socket behind; wayland-server's lock protocol makes
//! that harmless on the next bind.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_winit::shutdown::ShutdownFlag;
use parhelion_backend_winit::NestedBackend;
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::{ClientKey, SceneThread, SurfaceId, Transform};
use winit::event_loop::{ControlFlow, EventLoop};

/// Initial window size for the nested backend. Not a golden, not a constraint —
/// just a comfortable default; the window is resizable and the output follows it.
/// The DRM backend ignores it entirely and uses the connector's mode.
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

/// The command line, parsed once.
struct Args {
    /// Serve the socket with no window and no hardware.
    headless: bool,
    /// Drive real hardware through DRM/KMS from a TTY.
    drm: bool,
    /// An explicit DRM device, for machines with more than one card.
    drm_device: Option<PathBuf>,
    /// An explicit socket path instead of `$XDG_RUNTIME_DIR/wayland-N`.
    socket: Option<String>,
    /// Shut down automatically after this many seconds.
    exit_after: Option<Duration>,
}

/// Parse the arguments. Both `--flag value` and `--flag=value` are accepted,
/// because a first TTY run is the wrong moment to discover which form a
/// compositor wanted.
fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Look up a flag's value in either spelling.
    let value = |name: &str| -> Option<String> {
        let prefix = format!("{name}=");
        argv.iter()
            .find_map(|a| a.strip_prefix(&prefix).map(str::to_string))
            .or_else(|| {
                argv.iter()
                    .position(|a| a == name)
                    .and_then(|i| argv.get(i + 1))
                    .cloned()
            })
    };

    let exit_after = value("--exit-after").and_then(|s| match s.parse::<u64>() {
        Ok(secs) => Some(Duration::from_secs(secs)),
        Err(_) => {
            eprintln!("parhelion-dev: --exit-after wants a whole number of seconds; ignoring '{s}'");
            None
        }
    });

    Args {
        headless: argv.iter().any(|a| a == "--headless"),
        drm: argv.iter().any(|a| a == "--drm"),
        drm_device: value("--drm-device").map(PathBuf::from),
        socket: value("--socket"),
        exit_after,
    }
}

fn main() {
    let args = parse_args();

    // Signal handlers first: a compositor that will not shut down cleanly is
    // worth knowing about before it binds anything — and, with `--drm`, before it
    // takes the screen.
    let shutdown = ShutdownFlag::new();
    if let Err(e) = shutdown.install_signal_handlers() {
        eprintln!("parhelion-dev: could not install signal handlers ({e}); shutdown may litter");
    }

    // `--exit-after`: the escape hatch that needs no keyboard, which matters on
    // metal, where there is no keyboard yet (M2 T2). A plain thread raising the
    // same flag a signal would, so the exit path is the one already tested.
    if let Some(after) = args.exit_after {
        let flag = shutdown.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            println!("parhelion-dev: --exit-after elapsed; shutting down");
            flag.raise();
        });
    }

    // Canonical state and the protocol frontend — the same two pieces every test
    // builds, wired the same way, whichever backend follows.
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());

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
    let bound = match &args.socket {
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
            if args.headless {
                std::process::exit(1);
            }
        }
    }

    // --drm: the metal. The backend discovers the mode, tells `wl_output` the
    // truth about it, builds the compositor at that size, and runs until the
    // shutdown flag goes up. It owns its own render loop, because only it knows
    // how big the screen is.
    if args.drm {
        println!("parhelion-dev: DRM backend — NO INPUT and NO CURSOR yet (both are M2 T2)");
        let result = parhelion_backend_drm::run(
            scene.handle(),
            &host,
            CLEAR,
            args.drm_device,
            shutdown.shared(),
        );
        if let Err(e) = result {
            eprintln!("parhelion-dev: {e}");
            drop(host);
            std::process::exit(1);
        }
        // Dropping `host` stops the dispatch thread, which drops the listening
        // socket, which unlinks the socket and its lock file.
        drop(host);
        return;
    }

    // The two software backends share a render loop over the CPU compositor at a
    // fixed initial size; the nested one resizes it with its window.
    let mut render = RenderLoop::new(scene.handle(), CpuCompositor::new(WIDTH, HEIGHT, CLEAR))
        .with_presenter(host.frame_presenter());
    host.set_output_size(WIDTH, HEIGHT);

    if args.headless {
        run_headless(&mut render, &shutdown);
        drop(host);
        return;
    }

    let event_loop = EventLoop::new().expect("create the winit event loop");
    // Poll rather than Wait: the nested backend has no frame scheduler (the DRM
    // backend's vblank is the first real one, and the scheduler proper is M2 T3),
    // so frames are produced as fast as the loop turns. Honest for a development
    // backend, wrong for a real one — and said so here rather than discovered
    // later.
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
