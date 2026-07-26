//! Parhelion on the metal: DRM/KMS atomic commits into dumb buffers, under a
//! libseat session, surviving VT switches (M2 T1).
//!
//! Governing docs: `docs/scene_graph_v1.md` §13 (the canonical description of
//! this backend), `docs/plans/m2_tasks.md` T1, `docs/CORE-BOUNDARY.md` §3 C1 and
//! §7. The decision-log entries are dated 2026-07-26 under "The metal".
//!
//! # What this backend is
//!
//! The smallest honest path from a composited `Frame` to glass:
//!
//! - a **libseat session**, so the compositor runs as an ordinary user and
//!   survives VT switches instead of holding the machine hostage;
//! - **atomic** KMS commits — the modern API, and what CORE-BOUNDARY C1 names;
//! - **dumb buffers** — plain CPU memory any driver can scan out, because the
//!   renderer is still the CPU compositor and pretending otherwise would be
//!   dishonest. The GPU arrives deliberately in T4–T6;
//! - the **first connected connector at its preferred mode**, whose real
//!   geometry and refresh become what `wl_output` advertises.
//!
//! # What it is not, yet
//!
//! No cursor plane and **no input** — libinput and T-input are T2, so on metal
//! the keyboard is silent and there is no pointer. No frame scheduler
//! (render-as-late-as-possible) or `presentation-time`: T3. No GPU, no dmabuf,
//! no explicit sync: T4–T6. No multi-output, no hotplug, no suspend/resume: M9.
//! No plane offload beyond the primary: M5.
//!
//! # Threads (CORE-BOUNDARY §7)
//!
//! ```text
//!   T-scene ──Snapshot──▶ T-render ──XRGB8888 bytes──▶ T-commit ──atomic──▶ CRTC
//!   (canonical            (composites,                (owns the DRM fd,      │
//!    state, C4)            the CPU                     the session, the      │
//!                          compositor)                 vblank source)        │
//!                              ▲                              │              │
//!                              └───────── tick ◀──────────────┴── vblank ◀───┘
//! ```
//!
//! Two threads are spawned here and they are named — `parhelion-commit` and
//! `parhelion-render` — so that "which thread owns this?" is answerable from a
//! `gdb` backtrace or `htop`, not only from a document.
//!
//! # The seam
//!
//! Smithay supplies the session, the DRM device, the atomic surface, and the
//! vblank event source. It does **not** supply a renderer, a desktop layer, or
//! an allocator here: `smithay::backend::renderer` appears nowhere in this crate
//! or the workspace, `backend_gbm` and `backend_egl` are not enabled, and the
//! dumb buffers go straight through `smithay::reexports::drm`. That is the
//! consume/bypass split the threading-fit decision drew, kept at the one place it
//! was most likely to be lost by accident.

mod buffer;
mod commit;
pub mod mode;
pub mod present;
mod render;

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Instant;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::SceneHandle;
use smithay::reexports::calloop::channel::channel as calloop_channel;

/// A failure to get onto the metal, with the thing that was being attempted.
///
/// Startup on a TTY has no window to put an error in and no scrollback worth
/// relying on, so every failure carries both halves: what we were doing and what
/// the system said. "Cannot open a libseat session: no such file or directory"
/// is a bug report; "Error(2)" is a wasted evening.
#[derive(Debug)]
pub struct Error {
    /// What was being attempted, phrased as an infinitive.
    context: String,
    /// What went wrong, as the underlying error described it.
    detail: String,
}

impl Error {
    /// Build an error from what was attempted and what the system said.
    fn new(context: impl Into<String>, detail: impl fmt::Display) -> Self {
        Error {
            context: context.into(),
            detail: detail.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot {}: {}", self.context, self.detail)
    }
}

impl std::error::Error for Error {}

/// The output the backend actually found — the real mode, as the connector
/// reports it.
///
/// This is what retires T7's 60 Hz claim: the numbers here come from the mode
/// line, and `wl_output` advertises them verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputMode {
    /// Visible width in pixels.
    pub width: u32,
    /// Visible height in pixels.
    pub height: u32,
    /// Refresh in millihertz, computed from the mode's timings.
    pub refresh_mhz: i32,
    /// Connector name, e.g. `eDP-1`.
    pub connector: String,
    /// The DRM device the connector belongs to.
    pub device: PathBuf,
    /// The driver-chosen scanout stride in bytes — logged because a padded
    /// stride is the first thing to suspect when an image looks sheared.
    pub pitch: u32,
}

impl fmt::Display for OutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} on {} — {}x{} @ {}.{:03} Hz (stride {} bytes)",
            self.connector,
            self.device.display(),
            self.width,
            self.height,
            self.refresh_mhz / 1000,
            self.refresh_mhz % 1000,
            self.pitch
        )
    }
}

/// Run Parhelion on real hardware until `shutdown` is raised.
///
/// Blocks the calling thread for the whole session: it spawns T-commit and
/// T-render, waits for them, and returns when both have stopped. The caller's
/// only remaining job is to raise the shutdown flag (from a signal handler, or a
/// timer for `--exit-after`).
///
/// The sequence matters and is the reason this is one function rather than a
/// builder:
///
/// 1. T-commit starts and does **all** the fallible hardware work — session,
///    device, connector, mode, surface, buffers — reporting back either the mode
///    it found or a diagnostic. Nothing else has been built at that point, so a
///    failure costs nothing and prints cleanly.
/// 2. The real mode is handed to `wl_output` (`set_output_mode`) *before* the
///    compositor exists, so no client can ever see the placeholder size.
/// 3. The compositor is built at the mode's size and T-render starts.
///
/// (The session and the DRM device must be created *on* T-commit, not handed to
/// it: libseat's session and its event source are `Rc`-based and so are not
/// `Send`. Discovering that is why the mode travels back over a channel rather
/// than the device travelling forward.)
pub fn run(
    scene: SceneHandle,
    host: &ProtocolHost,
    clear: [u8; 4],
    device_path: Option<PathBuf>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Error> {
    // Commit → render: a tick, carrying a recycled pixel buffer. A plain channel:
    // T-render's whole job is to block on this.
    let (tick_tx, tick_rx) = mpsc::channel();
    // Render → commit: the converted frame. A calloop channel, because T-commit
    // waits on this *and* on vblanks *and* on session events in one loop.
    let (frame_tx, frame_rx) = calloop_channel();
    // Commit → here: the discovered mode, once, or the reason there is none.
    let (ready_tx, ready_rx) = mpsc::channel();

    let commit = thread::Builder::new()
        .name("parhelion-commit".into())
        .spawn({
            let scene = scene.clone();
            let shutdown = shutdown.clone();
            move || commit::run(scene, device_path, ready_tx, tick_tx, frame_rx, shutdown)
        })
        .map_err(|e| Error::new("spawn the commit thread", e))?;

    let mode = match ready_rx.recv() {
        Ok(Ok(mode)) => mode,
        Ok(Err(e)) => {
            let _ = commit.join();
            return Err(e);
        }
        Err(_) => {
            let _ = commit.join();
            return Err(Error::new(
                "start the commit thread",
                "it stopped before reporting an output",
            ));
        }
    };
    println!("parhelion-drm: {mode}");

    // The truthful mode, before any client can ask (T7's claim retired).
    host.set_output_mode(mode.width, mode.height, mode.refresh_mhz);

    let render_loop = RenderLoop::new(
        scene,
        CpuCompositor::new(mode.width, mode.height, clear),
    )
    .with_presenter(host.frame_presenter());

    let renderer = thread::Builder::new()
        .name("parhelion-render".into())
        .spawn(move || render::run(render_loop, tick_rx, frame_tx, Instant::now()))
        .map_err(|e| Error::new("spawn the render thread", e))?;

    // T-commit owns the tick sender, so its exit closes T-render's channel and
    // T-render falls out of its loop. Joining in this order needs no second flag.
    let _ = commit.join();
    let _ = renderer.join();
    Ok(())
}
