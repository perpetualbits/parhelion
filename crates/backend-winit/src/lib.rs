//! Parhelion nested backend — a desktop window that presents the composited
//! scene and feeds desktop input into the core (M1 T6).
//!
//! Governing design: `docs/scene_graph_v1.md` §11 (input and the nested
//! backend), `docs/plans/m1_tasks.md` T6, `CORE-BOUNDARY.md` §7. This crate is
//! the M1 interactive artifact: with it, "one window, honestly" becomes a window
//! Roland can look at, click on, and type into.
//!
//! # What this is not
//!
//! It is **not** `smithay::backend::winit`. That backend is welded to Smithay's
//! GLES renderer — the layer the threading-fit decision bypasses — so using it
//! would drag the whole graphics stack we deliberately supply ourselves back in.
//! Instead: raw winit for the window and its events, softbuffer to hand the
//! window CPU pixels, and the core's own `CpuCompositor` for the pixels
//! themselves. No renderer type from Smithay enters the tree (grep-verifiable).
//!
//! # The §7 deviation, stated plainly
//!
//! `CORE-BOUNDARY.md` §7 gives input its own thread (T-input) and presentation
//! another (T-commit/T-render). **winit does not allow that**: its event loop
//! must own the main thread, and it delivers input intake and window
//! presentation through that one loop. So in the nested backend, one thread does
//! both — it translates input into [`InputEvent`]s (which cross to the dispatch
//! thread as messages, exactly as a real T-input would) and drives the render
//! tick.
//!
//! This is a **bounded, recorded deviation, not a redesign**:
//!
//! - The *interface* a real T-input needs already exists and is already used —
//!   the funnel. When libinput arrives (M2) it produces the same `InputEvent`s
//!   from its own thread, and nothing downstream changes.
//! - Protocol objects are still touched only by the dispatch thread (§7's actual
//!   ownership rule is intact — this loop sends messages, it does not reach into
//!   the compositor's state).
//! - It applies to the *nested development* backend only. The DRM backend (M2)
//!   has no main-thread constraint and gets the real thread split.
//!
//! `docs/scene_graph_v1.md` §11.2 records the same thing in the canonical doc.
//!
//! # Structure
//!
//! - [`keycode`] — winit keys → evdev codes (unit-tested, unmapped keys counted).
//! - [`present`] — a composited `Frame` → softbuffer's pixel layout (unit-tested).
//! - [`shutdown`] — SIGINT/SIGTERM → an orderly exit, so the socket is unlinked.
//! - [`NestedBackend`] — the winit application: window, input translation, and
//!   the render tick. The only part that needs a display, and therefore the only
//!   part CI cannot exercise.

pub mod keycode;
pub mod present;
pub mod shutdown;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_core::input::{InputEvent, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::render::RenderLoop;
use parhelion_core::scene::SceneHandle;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::keycode::KeyTranslator;

/// The winit application: one window presenting the compositor's output, and one
/// event loop translating desktop input into the core's funnel.
///
/// Everything here is *plumbing*. The compositing is the core's, the protocol is
/// the core's, and the input semantics are the core's; this owns the window and
/// the translation, which is exactly the part that cannot live anywhere else.
pub struct NestedBackend {
    /// The scene, for snapshots and for damaging the output on resize.
    scene: SceneHandle,
    /// The protocol host, for feeding the input funnel.
    host: ProtocolHost,
    /// The render loop, ticked on each redraw.
    render: RenderLoop<CpuCompositor>,
    /// The window, once winit has created it (`None` before `resumed`).
    window: Option<Rc<Window>>,
    /// softbuffer's context and surface, created with the window.
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    /// Scratch buffer for the frame→softbuffer conversion, reused across frames.
    pixels: Vec<u32>,
    /// winit key → evdev translation, with its dropped-key counter.
    keys: KeyTranslator,
    /// When the backend started, so input and frame timestamps are monotonic ms
    /// from a fixed base (what the Wayland protocol asks for).
    start: Instant,
    /// The window title.
    title: String,
    /// Raised by a termination signal; polled each loop turn so the exit runs
    /// through winit's normal path and every `Drop` (including the listening
    /// socket's) gets to run.
    shutdown: crate::shutdown::ShutdownFlag,
}

impl NestedBackend {
    /// Build the backend over an already-running scene, host, and render loop.
    /// Nothing is created until winit calls `resumed`, which is where a window
    /// may first legally exist.
    pub fn new(
        scene: SceneHandle,
        host: ProtocolHost,
        render: RenderLoop<CpuCompositor>,
        title: impl Into<String>,
        shutdown: crate::shutdown::ShutdownFlag,
    ) -> Self {
        NestedBackend {
            scene,
            host,
            render,
            window: None,
            surface: None,
            pixels: Vec::new(),
            keys: KeyTranslator::new(),
            start: Instant::now(),
            title: title.into(),
            shutdown,
        }
    }

    /// Monotonic milliseconds since start — the timestamp every event and frame
    /// carries. Saturates rather than wrapping surprisingly; a session running
    /// long enough to overflow `u32` ms (~49 days) is M2's problem, and the
    /// protocol's own timestamps wrap anyway.
    fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    /// How many key events have been dropped for want of an evdev mapping. Read
    /// by the binary on exit so the gap is visible rather than silent.
    pub fn dropped_keys(&self) -> u64 {
        self.keys.dropped()
    }

    /// Produce one frame and present it in the window: tick the render loop
    /// (which snapshots the scene, composites, and fires frame callbacks), then
    /// blit the retained frame through softbuffer.
    fn redraw(&mut self) {
        let time = self.now_ms();
        self.render.tick(time);

        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };
        let frame = self.render.compositor().frame();
        let (w, h) = (frame.width(), frame.height());
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return; // a zero-sized window (minimised): nothing to present
        };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        present::frame_to_argb(frame, &mut self.pixels);
        if let Ok(mut buffer) = surface.buffer_mut() {
            buffer.copy_from_slice(&self.pixels);
            let _ = buffer.present();
        }
        // Keep animating: the M1 render loop has no frame scheduler (that is the
        // DRM backend's job in M2), so the window asks for the next frame itself.
        window.request_redraw();
    }
}

impl ApplicationHandler for NestedBackend {
    /// Create the window and its softbuffer surface. winit calls this once at
    /// startup (and again after a suspend on platforms that have one).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let frame = self.render.compositor().frame();
        let attrs = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(frame.width(), frame.height()));
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("create the nested backend's window"),
        );
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        window.request_redraw();
        self.window = Some(window);
        self.surface = Some(surface);
    }

    /// Translate one window event: input into the funnel, resize into an output
    /// resize, redraw into a frame.
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // A termination signal ends the loop the same way closing the window
        // does: `exit` unwinds to `run_app`'s return, everything drops, and the
        // socket unlinks itself (T7).
        if self.shutdown.is_raised() {
            event_loop.exit();
            return;
        }

        let time_ms = self.now_ms();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::RedrawRequested => self.redraw(),

            // The output changed size. The compositor resizes (which forces its
            // next frame to be a full repaint — its retained pixels describe the
            // old geometry), and the scene is told everything is damaged, so both
            // halves of the incremental-rendering contract agree.
            // (A zero-sized resize — a minimised window — is ignored: there is no
            // output to composite into, and `Frame::new(0, 0)` would be a lie.)
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                self.render.compositor_mut().resize(size.width, size.height);
                self.scene.mutate(|s| s.damage_full());
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // Repeats are the client's business (the protocol makes key
                // repeat client-side), so synthetic repeat events are dropped
                // here rather than delivered twice.
                if event.repeat {
                    return;
                }
                let winit::keyboard::PhysicalKey::Code(code) = event.physical_key else {
                    return; // an unidentified physical key: nothing to translate
                };
                if let Some(evdev) = self.keys.translate(code) {
                    self.host.input(InputEvent::Key {
                        code: evdev,
                        pressed: event.state == ElementState::Pressed,
                        time_ms,
                    });
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.host.input(InputEvent::PointerMotion {
                    x: position.x,
                    y: position.y,
                    time_ms,
                });
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(code) = button_code(button) {
                    self.host.input(InputEvent::PointerButton {
                        button: code,
                        pressed: state == ElementState::Pressed,
                        time_ms,
                    });
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // winit reports either whole lines (a stepped wheel) or pixels (a
                // touchpad). The protocol wants a scroll length; one line is
                // conventionally ~10 units, and only the stepped form carries
                // discrete steps.
                let (h, v, steps) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (-x as f64 * 10.0, -y as f64 * 10.0, -y as i32)
                    }
                    MouseScrollDelta::PixelDelta(p) => (-p.x, -p.y, 0),
                };
                self.host.input(InputEvent::PointerAxis {
                    horizontal: h,
                    vertical: v,
                    steps,
                    time_ms,
                });
            }

            _ => {}
        }
    }
}

/// winit mouse button → Linux `BTN_*` code. Extra buttons are dropped: the core
/// would forward any code, but inventing evdev numbers for them would be
/// guessing, and nothing in M1 needs them.
fn button_code(button: MouseButton) -> Option<u32> {
    match button {
        MouseButton::Left => Some(BTN_LEFT),
        MouseButton::Right => Some(BTN_RIGHT),
        MouseButton::Middle => Some(BTN_MIDDLE),
        _ => None,
    }
}
