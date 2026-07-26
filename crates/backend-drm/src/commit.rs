//! **T-commit** — the thread that owns the metal (M2 T1).
//!
//! Governing docs: `docs/CORE-BOUNDARY.md` §7 (T-commit's ownership) and §3 C1
//! (DRM/KMS is in the core); `docs/scene_graph_v1.md` §13.
//!
//! # What this thread owns, exclusively
//!
//! - the **libseat session** and its event source (VT switches arrive here),
//! - the **DRM device fd** and with it DRM master,
//! - the **atomic surface**: the CRTC, its connector, and the primary plane,
//! - the **two scanout buffers** and the framebuffers naming them,
//! - the **vblank event source**, and therefore the compositor's clock on metal.
//!
//! Nothing else in the process touches any of these. §7's rule — one owning
//! thread per resource, message passing across — is not a guideline here; it is
//! what makes it safe for T-render to be busy compositing while the display
//! engine is mid-scanout.
//!
//! # The frame cycle
//!
//! ```text
//!   vblank ──▶ T-commit: swap displayed/back, send a tick (with a recycled
//!              scratch buffer) ──▶ T-render: snapshot, composite, convert to
//!              XRGB8888 ──▶ T-commit: blit into the back buffer's mapping,
//!              atomic page-flip ──▶ vblank …
//! ```
//!
//! **The vblank is the tick.** M1's render loop was driven by whoever called
//! `tick()`; on metal it is driven by the display engine, which is what makes
//! the advertised refresh rate a description of what happens rather than a hope.
//! The headless and nested backends keep their own tick sources unchanged.
//!
//! # Why pixels cross the channel, and not the `Frame`
//!
//! The obvious design hands the composited `Frame` to T-commit. It cannot: the
//! CPU compositor **retains** its frame between frames and repaints only damaged
//! pixels (`scene_graph_v1.md` §9.4), so the frame cannot be moved out from under
//! it. What crosses instead is a recycled `Vec<u8>` holding the frame already
//! converted to the scanout format — work that has to happen anyway, done on the
//! thread that just touched every pixel. T-commit's remaining job is one memcpy
//! per row into the mapping. One conversion pass, one copy: the minimum for two
//! threads that must not share memory.
//!
//! # VT switching
//!
//! logind revokes our device access when the session goes away, and grants it
//! back on return. Both arrive as session events on this thread:
//!
//! - **pause** — stop committing, `DrmDevice::pause`. A frame in flight from
//!   T-render is **dropped, not queued**: it describes a screen nobody is
//!   looking at, and a queue that grows while paused is a queue that has to be
//!   bounded. Its buffer is kept for recycling.
//! - **resume** — reacquire the device, reset the surface's state (the other VT
//!   left the hardware however it liked), tell the scene **everything is damaged**
//!   so the next frame is a full repaint, and force the next commit to be a full
//!   modeset rather than a page-flip.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use parhelion_core::scene::SceneHandle;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmSurface, PlaneConfig, PlaneState};
use smithay::backend::session::libseat::{LibSeatSession, LibSeatSessionNotifier};
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::reexports::calloop::channel::{Channel, Event as ChannelEvent};
use smithay::reexports::calloop::{EventLoop, LoopSignal};
use smithay::reexports::drm::control::{
    connector, crtc, Device as ControlDevice, ModeFlags, ModeTypeFlags, ResourceHandles,
};
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Rectangle, Transform};

use crate::buffer::ScanoutBuffer;
use crate::mode::{self, ConnectorInfo, ModeInfo, ModeTiming};
use crate::{Error, OutputMode};

/// How long the loop sleeps between wakeups when nothing else is happening.
///
/// Two jobs: it is how often the shutdown flag is noticed (a signal handler only
/// sets a flag — `backend-winit`'s `shutdown` module explains why), and it is the
/// **watchdog** that restarts the frame cycle if a commit ever fails. Without the
/// second, one rejected atomic commit would leave the pipeline waiting for a
/// vblank that will never arrive, and the compositor would look hung.
const LOOP_TIMEOUT: Duration = Duration::from_millis(100);

/// Where to look for a KMS device when none was named on the command line.
///
/// Deliberately a fixed list rather than udev enumeration: `backend_udev` would
/// pull another Smithay feature (and libudev) into the build for something a
/// glob over eight paths does just as well at this milestone. Hotplug and
/// multi-GPU selection are M9, and that is when udev earns its place.
const CANDIDATE_DEVICES: [&str; 4] = [
    "/dev/dri/card0",
    "/dev/dri/card1",
    "/dev/dri/card2",
    "/dev/dri/card3",
];

/// A recycled pixel buffer travelling **T-commit → T-render**: "produce a frame,
/// and here is the allocation to put it in".
pub(crate) type Tick = Vec<u8>;

/// A converted frame travelling **T-render → T-commit**: `XRGB8888` bytes,
/// tightly packed, ready to be blitted into a scanout buffer's mapping.
pub(crate) type Presented = Vec<u8>;

// ==========================================================================
// Probing a device without committing to it.
// ==========================================================================

/// A raw DRM file descriptor we have opened but not yet decided to keep.
///
/// It exists so a rejected candidate device can be handed **back to libseat**
/// (`Session::close`) instead of merely dropped: libseat tracks the devices it
/// opened for us, and closing the descriptor behind its back leaves a stale
/// entry. `DrmDeviceFd` takes ownership of the fd and also acquires DRM master,
/// which is precisely what we do not want to do to a device we are only looking
/// at.
struct ProbeFd(std::os::fd::OwnedFd);

impl std::os::fd::AsFd for ProbeFd {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.0.as_fd()
    }
}
// The two marker traits that turn "something with a DRM fd" into a device the
// `drm` crate will issue mode-setting ioctls on. Both are empty.
impl smithay::reexports::drm::Device for ProbeFd {}
impl ControlDevice for ProbeFd {}

/// What a scan of one device found: the plain-data view the selection policy
/// consumes, alongside the DRM-side connector info it was derived from.
struct Scan {
    /// Policy input — see [`crate::mode`].
    infos: Vec<ConnectorInfo>,
    /// The same connectors, in the same order, as DRM reported them.
    connectors: Vec<connector::Info>,
    /// The device's resource handles, needed to resolve a CRTC later.
    resources: ResourceHandles,
}

/// Read every connector on `device` and reduce it to the selection policy's
/// vocabulary.
///
/// `force_probe` is on: it makes the kernel re-read EDID rather than trust a
/// cached answer, which is what makes "is a monitor plugged in?" reliable at
/// startup. It costs milliseconds, once.
fn scan(device: &impl ControlDevice) -> io::Result<Scan> {
    let resources = device.resource_handles()?;
    let mut infos = Vec::new();
    let mut connectors = Vec::new();

    for (index, handle) in resources.connectors().iter().enumerate() {
        let info = match device.get_connector(*handle, true) {
            Ok(info) => info,
            // A connector that fails to probe is skipped rather than fatal: the
            // machine may well have another one that works, and refusing to boot
            // because of a confused DP port would be a poor trade.
            Err(_) => continue,
        };

        let modes = info
            .modes()
            .iter()
            .enumerate()
            .map(|(mode_index, m)| {
                let (width, height) = m.size();
                let flags = m.flags();
                let timing = ModeTiming {
                    clock_khz: m.clock(),
                    htotal: m.hsync().2,
                    vtotal: m.vsync().2,
                    vscan: m.vscan(),
                    interlaced: flags.contains(ModeFlags::INTERLACE),
                    doublescan: flags.contains(ModeFlags::DBLSCAN),
                };
                ModeInfo {
                    index: mode_index,
                    width,
                    height,
                    preferred: m.mode_type().contains(ModeTypeFlags::PREFERRED),
                    // 0 rather than a guess when the timings are degenerate; the
                    // caller substitutes the default and says so out loud.
                    refresh_mhz: mode::refresh_mhz(&timing).unwrap_or(0),
                }
            })
            .collect();

        infos.push(ConnectorInfo {
            index,
            name: format!("{}-{}", info.interface().as_str(), info.interface_id()),
            connected: info.state() == connector::State::Connected,
            modes,
        });
        connectors.push(info);
    }

    Ok(Scan {
        infos,
        connectors,
        resources,
    })
}

/// Which CRTC should drive `connector`.
///
/// The connector's *current* CRTC is preferred when it has one: the firmware
/// already lit this panel on it, so we know the combination is valid and the
/// mode-set is less likely to blink. Otherwise the first CRTC any of the
/// connector's encoders can reach.
fn crtc_for(
    device: &impl ControlDevice,
    resources: &ResourceHandles,
    connector: &connector::Info,
) -> Option<crtc::Handle> {
    if let Some(encoder) = connector.current_encoder()
        && let Some(crtc) = device.get_encoder(encoder).ok().and_then(|e| e.crtc())
    {
        return Some(crtc);
    }
    connector
        .encoders()
        .iter()
        .filter_map(|handle| device.get_encoder(*handle).ok())
        .flat_map(|encoder| resources.filter_crtcs(encoder.possible_crtcs()))
        .next()
}

// ==========================================================================
// Setup: from a TTY to a surface.
// ==========================================================================

/// Everything the commit thread needs, assembled and validated.
struct Metal {
    session: LibSeatSession,
    session_notifier: LibSeatSessionNotifier,
    device: DrmDevice,
    drm_notifier: smithay::backend::drm::DrmDeviceNotifier,
    surface: DrmSurface,
    buffers: [ScanoutBuffer; 2],
    mode: OutputMode,
}

/// Open the session, find a usable device and connector, and build the atomic
/// surface and its scanout buffers.
///
/// Everything that can fail fails **here**, before a thread is committed to and
/// before `wl_output` has been told anything — and every failure names what was
/// being attempted, because the failure mode this replaces is a black screen on
/// a TTY with no way to read a backtrace.
fn setup(device_path: Option<PathBuf>) -> Result<Metal, Error> {
    // The session first: it is what grants us the device without being root, and
    // what will tell us about VT switches later.
    let (mut session, session_notifier) = LibSeatSession::new()
        .map_err(|e| Error::new("open a libseat session (is this a real TTY session?)", e))?;
    println!("parhelion-drm: seat {}", session.seat());

    let candidates: Vec<PathBuf> = match device_path {
        Some(path) => vec![path],
        None => CANDIDATE_DEVICES.iter().map(PathBuf::from).collect(),
    };

    // Probe each candidate in turn, keeping the first with something plugged in.
    // A rejected device is handed back to libseat rather than merely closed.
    let mut rejected = Vec::new();
    let mut chosen: Option<(PathBuf, ProbeFd, Scan, mode::Selection)> = None;
    for path in &candidates {
        if !path.exists() {
            continue;
        }
        let fd = match session.open(path, OFlags::RDWR | OFlags::CLOEXEC) {
            Ok(fd) => ProbeFd(fd),
            Err(e) => {
                rejected.push(format!("{}: cannot open ({e})", path.display()));
                continue;
            }
        };
        let found = match scan(&fd) {
            Ok(scan) => scan,
            Err(e) => {
                rejected.push(format!("{}: cannot read connectors ({e})", path.display()));
                let _ = session.close(fd.0);
                continue;
            }
        };

        // Hardware honesty (CLAUDE.md's technical-attitude rule): say what the
        // device actually reports, not what we hoped it would.
        for c in &found.infos {
            println!(
                "parhelion-drm: {} {} — {} connector: {} mode(s)",
                path.display(),
                c.name,
                if c.connected {
                    "connected"
                } else {
                    "disconnected"
                },
                c.modes.len()
            );
        }

        match mode::select_output(&found.infos) {
            Some(selection) => {
                chosen = Some((path.clone(), fd, found, selection));
                break;
            }
            None => {
                rejected.push(format!("{}: no connected connector", path.display()));
                let _ = session.close(fd.0);
            }
        }
    }

    let (path, fd, found, selection) = chosen.ok_or_else(|| {
        Error::new(
            "find a usable DRM device",
            if rejected.is_empty() {
                "no /dev/dri/card* device exists".to_string()
            } else {
                rejected.join("; ")
            },
        )
    })?;

    let connector = &found.connectors[selection.connector];
    let drm_mode = connector.modes()[selection.mode];
    let info = &found.infos[selection.connector];
    let mode_info = &info.modes[selection.mode];

    let crtc = crtc_for(&fd, &found.resources, connector).ok_or_else(|| {
        Error::new(
            "find a CRTC for the selected connector",
            format!("{} has no usable encoder", info.name),
        )
    })?;

    // Only now do we take ownership of the fd — and with it DRM master.
    let device_fd = DrmDeviceFd::new(DeviceFd::from(fd.0));
    let (mut device, drm_notifier) = DrmDevice::new(device_fd, true)
        .map_err(|e| Error::new(format!("open {} as a DRM device", path.display()), e))?;

    if !device.is_atomic() {
        return Err(Error::new(
            "use atomic mode-setting",
            format!(
                "{} exposes only the legacy KMS API. Parhelion commits atomically \
                 (CORE-BOUNDARY C1); a driver without atomic support is out of scope \
                 for M2 T1",
                path.display()
            ),
        ));
    }

    let surface = device
        .create_surface(crtc, drm_mode, &[connector.handle()])
        .map_err(|e| Error::new("create an atomic surface", e))?;

    let (width, height) = (mode_info.width as u32, mode_info.height as u32);
    let device_fd = device.device_fd().clone();
    let buffers = [
        ScanoutBuffer::new(&device_fd, width, height)
            .map_err(|e| Error::new("allocate the front scanout buffer", e))?,
        ScanoutBuffer::new(&device_fd, width, height)
            .map_err(|e| Error::new("allocate the back scanout buffer", e))?,
    ];

    // A refresh of 0 means the mode's timings were degenerate. Say so and fall
    // back rather than advertising a rate of zero to every client.
    let refresh_mhz = if mode_info.refresh_mhz > 0 {
        mode_info.refresh_mhz
    } else {
        println!(
            "parhelion-drm: {} reports timings that yield no refresh rate; \
             advertising {} mHz",
            info.name,
            parhelion_core::protocol::OUTPUT_REFRESH_MHZ
        );
        parhelion_core::protocol::OUTPUT_REFRESH_MHZ
    };

    let mode = OutputMode {
        width,
        height,
        refresh_mhz,
        connector: info.name.clone(),
        device: path,
        pitch: buffers[0].pitch(),
    };

    Ok(Metal {
        session,
        session_notifier,
        device,
        drm_notifier,
        surface,
        buffers,
        mode,
    })
}

// ==========================================================================
// The loop.
// ==========================================================================

/// T-commit's state: everything the event callbacks mutate.
struct CommitState {
    device: DrmDevice,
    surface: DrmSurface,
    buffers: [ScanoutBuffer; 2],
    width: u32,
    height: u32,
    /// Index of the buffer the CRTC is scanning out.
    displayed: usize,
    /// Index submitted in the flip we are waiting on.
    flipping: usize,
    /// A page-flip (or modeset) is in flight; its vblank has not arrived.
    flip_pending: bool,
    /// T-render has been ticked and has not answered yet. Keeps exactly one
    /// frame in flight — the whole of M2 T1's frame scheduling, and the honest
    /// amount until T3 builds a real one.
    awaiting_frame: bool,
    /// We hold the device (no other VT is foreground).
    active: bool,
    /// The next submission must be a full modeset, not a page-flip: true at
    /// startup and after every VT return.
    needs_modeset: bool,
    /// A pixel buffer to hand back to T-render with the next tick.
    spare: Option<Vec<u8>>,
    /// For `damage_full` on resume — the scene is canonical state (I-5) and is
    /// reached only by message (I-3).
    scene: SceneHandle,
    /// The tick edge to T-render.
    tick_tx: mpsc::Sender<Tick>,
    /// Stops the loop when a termination signal arrives.
    signal: LoopSignal,
    shutdown: Arc<AtomicBool>,
    /// Frames actually scanned out.
    frames_presented: u64,
    /// Atomic commits the kernel rejected — printed on the way out, because a
    /// compositor that silently drops frames is a compositor nobody can debug.
    commits_failed: u64,
    /// Session pause/resume pairs survived.
    vt_switches: u64,
}

impl CommitState {
    /// Ask T-render for a frame, if the pipeline is idle and we own the screen.
    ///
    /// Every path that could restart the cycle funnels through here, so "exactly
    /// one frame in flight" is a property of one function rather than of four
    /// callbacks agreeing with each other.
    fn request_frame(&mut self) {
        if !self.active || self.flip_pending || self.awaiting_frame {
            return;
        }
        let scratch = self.spare.take().unwrap_or_default();
        if self.tick_tx.send(scratch).is_ok() {
            self.awaiting_frame = true;
        } else {
            // T-render is gone; so is the reason for this thread to exist.
            self.signal.stop();
        }
    }

    /// A frame arrived from T-render: blit it into the back buffer and submit.
    fn on_frame(&mut self, pixels: Presented) {
        self.awaiting_frame = false;

        if !self.active {
            // Paused: the frame describes a screen nobody can see. Drop it, keep
            // its allocation. Bounded by construction — there is never a second.
            self.spare = Some(pixels);
            return;
        }

        let back = 1 - self.displayed;
        match self.buffers[back].write(&pixels, self.width, self.height) {
            Ok(rows) if rows == self.height => {}
            Ok(rows) => {
                eprintln!(
                    "parhelion-drm: scanout buffer took only {rows} of {} rows; \
                     skipping this frame",
                    self.height
                );
                self.spare = Some(pixels);
                self.commits_failed += 1;
                return;
            }
            Err(e) => {
                eprintln!("parhelion-drm: cannot map the scanout buffer ({e}); skipping this frame");
                self.spare = Some(pixels);
                self.commits_failed += 1;
                return;
            }
        }
        self.spare = Some(pixels);
        self.submit(back);
    }

    /// Submit buffer `index` to the CRTC: a page-flip normally, a full modeset
    /// when the hardware's state is not ours (startup, VT return).
    fn submit(&mut self, index: usize) {
        let plane = PlaneState {
            handle: self.surface.plane(),
            config: Some(PlaneConfig {
                // Source: the whole buffer, in buffer pixels.
                src: Rectangle::from_size((self.width as f64, self.height as f64).into()),
                // Destination: the whole CRTC. Equal by construction — the
                // buffers were allocated at the mode's size — so there is no
                // scaling, which is the only thing a dumb-buffer primary plane
                // can be relied on to do.
                dst: Rectangle::from_size((self.width as i32, self.height as i32).into()),
                transform: Transform::Normal,
                alpha: 1.0,
                // FB_DAMAGE_CLIPS is deliberately not used yet: it lets the
                // driver re-scan only changed rectangles, and the payoff is
                // plane offload (M5). Passing None means "all of it changed",
                // which is always correct and never a lie.
                damage_clips: None,
                fb: self.buffers[index].framebuffer(),
                // No fence: dumb buffers are CPU memory, the blit above already
                // completed, and there is nothing asynchronous to wait for.
                // Explicit sync (I-11) arrives with the GPU in T6.
                fence: None,
            }),
        };

        let result = if self.needs_modeset {
            self.surface.commit([plane], true)
        } else {
            self.surface.page_flip([plane], true)
        };

        match result {
            Ok(()) => {
                self.needs_modeset = false;
                self.flip_pending = true;
                self.flipping = index;
            }
            Err(e) => {
                // Do not retry in place: a failing commit retried in a tight loop
                // is a spin, and this thread is the one that must never spin. The
                // loop's timeout is the retry, at 10 Hz, with a modeset next time.
                eprintln!("parhelion-drm: atomic commit rejected ({e}); retrying with a modeset");
                self.commits_failed += 1;
                self.needs_modeset = true;
            }
        }
    }

    /// The flip completed and the new buffer is on screen.
    fn on_vblank(&mut self) {
        self.flip_pending = false;
        self.displayed = self.flipping;
        self.frames_presented += 1;
        self.request_frame();
    }

    /// logind took the session away (someone switched VT).
    fn on_pause(&mut self) {
        self.active = false;
        self.flip_pending = false;
        self.device.pause();
        self.vt_switches += 1;
        println!("parhelion-drm: session paused (VT switched away)");
    }

    /// logind gave the session back.
    fn on_activate(&mut self) {
        if let Err(e) = self.device.activate(true) {
            eprintln!("parhelion-drm: cannot reactivate the device ({e})");
            return;
        }
        if let Err(e) = self.surface.reset_state() {
            eprintln!("parhelion-drm: cannot reset the surface state ({e})");
        }
        // The other VT owned this screen; nothing about our retained frame or the
        // scanout buffers describes what is on it. Full damage means the next
        // frame is a complete repaint, and the modeset re-establishes the mode.
        self.scene.mutate(|scene| scene.damage_full());
        self.active = true;
        self.needs_modeset = true;
        println!("parhelion-drm: session resumed (VT switched back)");
        self.request_frame();
    }
}

/// Run T-commit. Returns when the shutdown flag is raised or the pipeline dies.
///
/// `ready_tx` carries the discovered mode (or the setup failure) back to the
/// thread that spawned this one, which is what lets `wl_output` be told the truth
/// before any client can ask.
pub(crate) fn run(
    scene: SceneHandle,
    device_path: Option<PathBuf>,
    ready_tx: mpsc::Sender<Result<OutputMode, Error>>,
    tick_tx: mpsc::Sender<Tick>,
    frames: Channel<Presented>,
    shutdown: Arc<AtomicBool>,
) {
    let metal = match setup(device_path) {
        Ok(metal) => {
            let _ = ready_tx.send(Ok(metal.mode.clone()));
            metal
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    let mut event_loop: EventLoop<CommitState> = match EventLoop::try_new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("parhelion-drm: cannot create the commit thread's event loop ({e})");
            return;
        }
    };
    let handle = event_loop.handle();

    // The session is active or we would not have got this far; confirm rather
    // than assume, so a race with a VT switch during startup resolves the safe
    // way (no commits until logind says we may).
    //
    // After this the `LibSeatSession` handle itself is dropped, and that is
    // correct rather than careless: Smithay's session handle is a *weak*
    // reference — the **notifier** owns the seat. Keeping a second handle alive
    // would suggest an ownership this thread does not have, and the loop learns
    // about activity from session *events*, not by asking.
    let active = metal.session.is_active();
    drop(metal.session);

    let mut state = CommitState {
        device: metal.device,
        surface: metal.surface,
        buffers: metal.buffers,
        width: metal.mode.width,
        height: metal.mode.height,
        displayed: 0,
        flipping: 0,
        flip_pending: false,
        awaiting_frame: false,
        active,
        needs_modeset: true,
        spare: None,
        scene,
        tick_tx,
        signal: event_loop.get_signal(),
        shutdown,
        frames_presented: 0,
        commits_failed: 0,
        vt_switches: 0,
    };

    // Vblank: the clock.
    if let Err(e) = handle.insert_source(metal.drm_notifier, |event, _meta, state| match event {
        DrmEvent::VBlank(_crtc) => state.on_vblank(),
        DrmEvent::Error(e) => eprintln!("parhelion-drm: DRM event error ({e})"),
    }) {
        eprintln!("parhelion-drm: cannot watch for vblank events ({e})");
        return;
    }

    // Session: VT switches.
    if let Err(e) = handle.insert_source(metal.session_notifier, |event, _, state| match event {
        SessionEvent::PauseSession => state.on_pause(),
        SessionEvent::ActivateSession => state.on_activate(),
    }) {
        eprintln!("parhelion-drm: cannot watch for session events ({e})");
        return;
    }

    // Frames from T-render.
    if let Err(e) = handle.insert_source(frames, |event, _, state| match event {
        ChannelEvent::Msg(pixels) => state.on_frame(pixels),
        // T-render dropped its sender: the compositor has no source of pixels.
        ChannelEvent::Closed => state.signal.stop(),
    }) {
        eprintln!("parhelion-drm: cannot watch for rendered frames ({e})");
        return;
    }

    // Prime the cycle: the first tick produces the first frame, and that frame's
    // submission is the initial modeset. There is no separate "show something
    // black first" step, because there is nothing to show it for.
    state.request_frame();

    let result = event_loop.run(Some(LOOP_TIMEOUT), &mut state, |state| {
        if state.shutdown.load(Ordering::Relaxed) {
            state.signal.stop();
            return;
        }
        // The watchdog (see LOOP_TIMEOUT): if the cycle is idle while we own the
        // screen, restart it. `request_frame` is a no-op unless it genuinely is.
        state.request_frame();
    });
    if let Err(e) = result {
        eprintln!("parhelion-drm: commit loop failed ({e})");
    }

    println!(
        "parhelion-drm: {} frame(s) presented, {} commit(s) rejected, {} VT switch(es)",
        state.frames_presented, state.commits_failed, state.vt_switches
    );
    // Dropping the state drops the buffers, the surface, and finally the device
    // fd — which is what releases DRM master and lets the kernel give the console
    // back. Nothing here needs to happen in a signal handler.
}

/// Sanity guard: the file the loop opens is a real path, not a name we invented.
/// Cheap, and it is the sort of typo that only shows up on the machine that has
/// no display to print to.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Every candidate device path is absolute and under `/dev/dri`. A relative
    /// path here would open something in the working directory, which on a
    /// compositor running from a TTY is nobody's idea of a good time.
    #[test]
    fn candidate_device_paths_are_under_dev_dri() {
        for path in CANDIDATE_DEVICES {
            let path = Path::new(path);
            assert!(path.is_absolute(), "{} is not absolute", path.display());
            assert_eq!(path.parent(), Some(Path::new("/dev/dri")));
        }
    }
}
