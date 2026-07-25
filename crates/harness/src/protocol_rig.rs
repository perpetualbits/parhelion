//! The protocol test rig: drive a real [`ProtocolHost`] with a scripted,
//! in-process Wayland client and assert on both sides of the seam.
//!
//! Governing design: `docs/smithay_threading_spike.md` §5.2 (the rig is the
//! spike's split experiment promoted — in-process socketpair, scripted
//! `wayland-client`, fully deterministic, no external sockets or processes) and
//! the decision "2026-07-24 — Smithay threading fit" (the ProtocolHost seam
//! this exercises).
//!
//! # How a test reads
//!
//! ```no_run
//! # use parhelion_harness::protocol_rig::ScriptedClient;
//! # use parhelion_core::protocol::ProtocolHost;
//! # use parhelion_core::scene::SceneThread;
//! # use std::os::unix::net::UnixStream;
//! let scene = SceneThread::spawn();            // owns the canonical scene
//! let host = ProtocolHost::new(scene.handle()); // publishes lifecycle into it
//! let (server_end, client_end) = UnixStream::pair().unwrap();
//! host.add_client(server_end);                 // the accept seam
//! let mut client = ScriptedClient::connect(client_end);
//! let surface = client.create_surface();
//! client.commit(&surface);
//! client.roundtrip();                          // block until the server saw it
//! let h = scene.handle();
//! assert_eq!(h.query(|s| s.surface_count()), 1); // query the scene
//! ```
//!
//! Determinism: the client's `roundtrip` returns only after the server has
//! processed its requests, and protocol ordering guarantees the resulting
//! scene messages are already enqueued — so a subsequent `scene.query(...)`
//! observes them without any sleep. Disconnect (no round-trip to sync on) uses
//! [`SceneHandle::wait_until`](parhelion_core::scene::SceneHandle::wait_until)
//! instead.
//!
//! # Windows, not surfaces (T5)
//!
//! A bare `wl_surface` is never displayed, so a test that wants *visible content*
//! asks for a window: [`ScriptedClient::map_toplevel`] performs the entire
//! xdg-shell dance — create surface → `xdg_surface` → `xdg_toplevel` → initial
//! commit → configure → ack → draw → commit → mapped — in one call, and
//! [`ScriptedClient::map_toplevel_with`] lets the test paint between the
//! configure and the mapping commit. The individual steps stay exposed so the
//! conformance tests can break the dance deliberately, and
//! [`ScriptedClient::expect_protocol_error`] asserts on the *specific* error the
//! server posts.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
use wayland_protocols::xdg::shell::client::xdg_positioner::XdgPositioner;
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

/// Client-side dispatch state: everything the scripted client *observes*. It
/// sends requests directly, but the events it must see — frame callbacks (T2),
/// buffer releases (T3), and the xdg configure/ping traffic (T5) — land here for
/// tests to assert on.
#[derive(Default)]
struct App {
    /// Timestamps carried by every `wl_callback.done` received, in arrival order
    /// — the reverse-direction evidence a test asserts on.
    frame_dones: Vec<u32>,
    /// Count of `wl_buffer.release` events received — the T3 evidence that the
    /// compositor copied-and-released immediately (single-buffer clients rely on
    /// this to reuse the buffer).
    buffer_releases: u32,
    /// `xdg_surface.configure` serials received and not yet consumed by
    /// [`ScriptedClient::wait_for_configure`]. A queue, not a latest-value slot,
    /// so a test that expects exactly one configure can prove exactly one arrived.
    xdg_configures: VecDeque<u32>,
    /// Sizes carried by `xdg_toplevel.configure`, in arrival order. `(0, 0)` is
    /// the compositor saying "you choose" — what Parhelion's C10 fallback sends.
    toplevel_configures: Vec<(i32, i32)>,
    /// Count of `xdg_wm_base.ping` events received. The rig answers each one with
    /// `pong` immediately (a well-behaved client), so this is the client-side half
    /// of the liveness handshake.
    pings: u32,
    /// Every input-related event received, in arrival order (T6). One list, not
    /// one per kind, because **order is the thing most worth asserting**: an
    /// `enter` must precede any key or button, and a `leave` must precede the
    /// next surface's `enter`.
    input_events: Vec<SeatEvent>,
    /// The xkb keymap the compositor sent, as text.
    keymap: Option<String>,
    /// Capabilities advertised on `wl_seat`, as the raw bitfield.
    seat_capabilities: u32,
    /// What the compositor said about its output (T7): mode size, scale, and how
    /// many `done` events closed an atomic batch of output state.
    output_mode: Option<(i32, i32, i32)>,
    output_scale: i32,
    output_done: u32,
    /// `wl_surface.enter`/`leave` — which surfaces the compositor says are on an
    /// output, in arrival order (by client-side object id).
    surface_outputs: Vec<(u32, bool)>,
    /// `xdg_output`'s logical geometry, which is what a client actually lays
    /// itself out against (it is scale-independent, unlike `wl_output.mode`).
    xdg_logical_size: Option<(i32, i32)>,
    xdg_logical_position: Option<(i32, i32)>,
    /// The clipboard offer the compositor last handed this client, and the mime
    /// types it advertised. `None` after a `selection(null)` — which is how a
    /// client learns the clipboard was cleared.
    selection_offer: Option<WlDataOffer>,
    /// Mime types announced for the offer above, in arrival order.
    offer_mimes: Vec<String>,
    /// How many `selection` events have arrived (including the null one), so a
    /// test can wait on "the clipboard changed" rather than on a timer.
    selection_events: u32,
    /// Set when one of *this client's* data sources was cancelled — the event a
    /// client gets when its selection is replaced, or its drag refused.
    source_cancelled: u32,
    /// `send` requests the compositor forwarded to this client's source: the
    /// other side wants the bytes. Recorded as (mime type, fd) so the test can
    /// answer them.
    send_requests: Vec<(String, std::os::fd::OwnedFd)>,
}

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlCompositor, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wayland_client::protocol::wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for App {
    /// Record `enter`/`leave`: which of this client's surfaces the compositor
    /// says are on an output (T7). `true` is an enter.
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wayland_client::protocol::wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = surface.id().protocol_id();
        match event {
            wayland_client::protocol::wl_surface::Event::Enter { .. } => {
                state.surface_outputs.push((id, true))
            }
            wayland_client::protocol::wl_surface::Event::Leave { .. } => {
                state.surface_outputs.push((id, false))
            }
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for App {
    /// Record the timestamp from each frame callback's `done`. `wl_callback` is
    /// one-shot: `done` is the only event and fires at most once per callback.
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { callback_data } = event {
            state.frame_dones.push(callback_data);
        }
    }
}

impl Dispatch<WlShm, ()> for App {
    /// `wl_shm` emits `format` events advertising supported formats; the rig binds
    /// the formats it uses directly, so it ignores them.
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wayland_client::protocol::wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for App {
    /// Count `release` events — the compositor is done reading the buffer and the
    /// client may reuse it.
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.buffer_releases += 1;
        }
    }
}

impl Dispatch<XdgWmBase, ()> for App {
    /// Answer `ping` with `pong` at once — the rig models a responsive client —
    /// and count it so a test can assert the ping actually arrived.
    fn event(
        state: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            state.pings += 1;
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for App {
    /// Queue each `configure` serial. The client must `ack_configure` one of
    /// these before it may commit a buffer — the rig never acks automatically,
    /// because *when* it acks is exactly what the conformance tests vary.
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state.xdg_configures.push_back(serial);
        }
    }
}

impl Dispatch<XdgToplevel, ()> for App {
    /// Record the suggested size from each `configure`. `close` needs no handling
    /// in M1 (nothing asks a window to close yet).
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            state.toplevel_configures.push((width, height));
        }
    }
}

impl Dispatch<WlOutput, ()> for App {
    /// Record the output's advertised state. `mode`, `scale`, and `geometry`
    /// arrive as a batch closed by `done`, which is what a client waits for
    /// before trusting any of them.
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Mode {
                width,
                height,
                refresh,
                ..
            } => state.output_mode = Some((width, height, refresh)),
            wl_output::Event::Scale { factor } => state.output_scale = factor,
            wl_output::Event::Done => state.output_done += 1,
            _ => {}
        }
    }
}

impl Dispatch<WlSubcompositor, ()> for App {
    /// Emits no events; it hands out `wl_subsurface` objects.
    fn event(
        _: &mut Self,
        _: &WlSubcompositor,
        _: <WlSubcompositor as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSubsurface, ()> for App {
    /// Emits no events either. The rig creates one only to prove the compositor
    /// refuses it out loud (M2 T0's tripwire); it never gets to be useful.
    fn event(
        _: &mut Self,
        _: &WlSubsurface,
        _: <WlSubsurface as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDeviceManager, ()> for App {
    /// The manager emits no events; it hands out devices and sources.
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for App {
    /// The clipboard arriving. `data_offer` introduces a new offer object,
    /// `selection` says which offer (if any) is now the clipboard.
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // A fresh offer object; its mime types follow as `offer` events.
            wl_data_device::Event::DataOffer { id } => {
                state.selection_offer = Some(id);
                state.offer_mimes.clear();
            }
            wl_data_device::Event::Selection { id } => {
                state.selection_events += 1;
                // `None` means the clipboard was cleared.
                if id.is_none() {
                    state.selection_offer = None;
                    state.offer_mimes.clear();
                }
            }
            _ => {}
        }
    }

    // `data_offer` is one of the few Wayland events that *creates* an object, so
    // the client library must be told what to build and with what user data.
    // Without this the queue panics on the first clipboard offer.
    wayland_client::event_created_child!(App, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for App {
    /// Each `offer` event announces one mime type the source can produce.
    fn event(
        state: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.offer_mimes.push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, ()> for App {
    /// `send` means somebody wants our bytes; `cancelled` means this source is no
    /// longer the selection (replaced, or its drag refused).
    fn event(
        state: &mut Self,
        _: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                state.send_requests.push((mime_type, fd));
            }
            wl_data_source::Event::Cancelled => state.source_cancelled += 1,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputManagerV1, ()> for App {
    /// The manager emits no events; it exists to hand out `xdg_output` objects.
    fn event(
        _: &mut Self,
        _: &ZxdgOutputManagerV1,
        _: <ZxdgOutputManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgOutputV1, ()> for App {
    /// Record the logical geometry — the scale-independent size and position a
    /// client lays itself out against.
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                state.xdg_logical_size = Some((width, height))
            }
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                state.xdg_logical_position = Some((x, y))
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for App {
    /// Record the advertised capabilities; the name event is ignored.
    fn event(
        state: &mut Self,
        _: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            state.seat_capabilities = capabilities.into();
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    /// Record keymap, focus changes, keys, and modifiers — in one ordered list,
    /// because the ordering is half of what the tests check.
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // The keymap arrives as a file descriptor holding xkb text. A real
            // client mmaps it; reading is equivalent here and needs no unsafe.
            wl_keyboard::Event::Keymap { fd, size, .. } => {
                let file = File::from(fd);
                let mut text = String::new();
                Read::take(file, size as u64).read_to_string(&mut text).ok();
                // The map is NUL-terminated on the wire; trim so callers can
                // compare against plain text.
                state.keymap = Some(text.trim_end_matches('\0').to_string());
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                state.input_events.push(SeatEvent::KeyboardEnter {
                    surface: surface.id().protocol_id(),
                });
            }
            wl_keyboard::Event::Leave { surface, .. } => {
                state.input_events.push(SeatEvent::KeyboardLeave {
                    surface: surface.id().protocol_id(),
                });
            }
            wl_keyboard::Event::Key {
                key, state: s, serial, ..
            } => {
                state.input_events.push(SeatEvent::Key {
                    key,
                    pressed: matches!(s, WEnum::Value(wl_keyboard::KeyState::Pressed)),
                    serial,
                });
            }
            wl_keyboard::Event::Modifiers { mods_depressed, .. } => {
                state.input_events.push(SeatEvent::Modifiers {
                    depressed: mods_depressed,
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for App {
    /// Record crossings, motion, buttons, and axis events in arrival order.
    /// `frame` is ignored: it groups the events above, and the rig asserts on the
    /// events themselves.
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => state.input_events.push(SeatEvent::PointerEnter {
                surface: surface.id().protocol_id(),
                x: surface_x,
                y: surface_y,
            }),
            wl_pointer::Event::Leave { surface, .. } => {
                state.input_events.push(SeatEvent::PointerLeave {
                    surface: surface.id().protocol_id(),
                })
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.input_events.push(SeatEvent::PointerMotion {
                x: surface_x,
                y: surface_y,
            }),
            wl_pointer::Event::Button {
                button,
                state: s,
                serial,
                ..
            } => state.input_events.push(SeatEvent::PointerButton {
                button,
                pressed: matches!(s, WEnum::Value(wl_pointer::ButtonState::Pressed)),
                serial,
            }),
            wl_pointer::Event::Axis { axis, value, .. } => {
                state.input_events.push(SeatEvent::PointerAxis {
                    axis: u32::from(axis),
                    value,
                })
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgPositioner, ()> for App {
    /// `xdg_positioner` is write-only — it emits no events. The rig creates one
    /// solely to reach `xdg_surface.get_popup` in the double-role error test.
    fn event(
        _: &mut Self,
        _: &XdgPositioner,
        _: <XdgPositioner as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgPopup, ()> for App {
    /// Popups are out of scope for M1 — Parhelion dismisses them on creation.
    /// The rig only creates one to provoke the second-role protocol error, and
    /// that error arrives before any popup event could.
    fn event(
        _: &mut Self,
        _: &XdgPopup,
        _: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// One seat event the scripted client received, flattened for assertions (T6).
///
/// Surfaces are identified by their client-side object id rather than the proxy,
/// so a test can say "the enter went to *that* window" without holding protocol
/// objects in its expectations.
#[derive(Debug, Clone, PartialEq)]
pub enum SeatEvent {
    /// `wl_keyboard.enter` — this surface now has keyboard focus.
    KeyboardEnter {
        /// Object id of the focused surface.
        surface: u32,
    },
    /// `wl_keyboard.leave` — this surface lost keyboard focus.
    KeyboardLeave {
        /// Object id of the surface that lost focus.
        surface: u32,
    },
    /// `wl_keyboard.key` — a key changed state. `key` is the **evdev** code.
    Key {
        /// evdev keycode.
        key: u32,
        /// Whether this was a press.
        pressed: bool,
        /// The event's serial, for monotonicity assertions.
        serial: u32,
    },
    /// `wl_keyboard.modifiers` — the modifier state changed.
    Modifiers {
        /// Depressed (currently held) modifier mask.
        depressed: u32,
    },
    /// `wl_pointer.enter` — the cursor entered this surface, at surface-local
    /// coordinates.
    PointerEnter {
        /// Object id of the entered surface.
        surface: u32,
        /// Surface-local x.
        x: f64,
        /// Surface-local y.
        y: f64,
    },
    /// `wl_pointer.leave` — the cursor left this surface.
    PointerLeave {
        /// Object id of the surface left.
        surface: u32,
    },
    /// `wl_pointer.motion` — the cursor moved within the focused surface.
    PointerMotion {
        /// Surface-local x.
        x: f64,
        /// Surface-local y.
        y: f64,
    },
    /// `wl_pointer.button` — a button changed state. `button` is a `BTN_*` code.
    PointerButton {
        /// `BTN_*` code.
        button: u32,
        /// Whether this was a press.
        pressed: bool,
        /// The event's serial.
        serial: u32,
    },
    /// `wl_pointer.axis` — scrolling. `axis` is 0 for vertical, 1 for horizontal
    /// (the protocol's own numbering).
    PointerAxis {
        /// Axis number, per `wl_pointer.axis`.
        axis: u32,
        /// Scroll amount.
        value: f64,
    },
}

/// Read a pipe to EOF if the writer has finished, else `None`. The pipe is set
/// non-blocking so a test never hangs waiting for a transfer that is not coming.
fn try_read_to_end(fd: &std::io::PipeReader) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut reader = fd.try_clone().ok()?;
    let mut buf = Vec::new();
    match reader.read_to_end(&mut buf) {
        Ok(_) if !buf.is_empty() => Some(buf),
        _ => None,
    }
}

/// A protocol error the server sent this client, flattened so tests need not
/// depend on `wayland-client`'s types. Asserting on [`code`](Self::code) — not
/// merely "the client got disconnected" — is what makes an error test mean
/// something: the wrong error for the right reason is still a bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigProtocolError {
    /// The error code, interpreted against `interface`'s `error` enum.
    pub code: u32,
    /// The interface of the object the error was posted on.
    pub interface: String,
    /// The server's human-readable message (diagnostics only; never asserted on).
    pub message: String,
}

/// A mapped xdg toplevel and the objects behind it, as returned by
/// [`ScriptedClient::map_toplevel`]. Keeping the pool and buffer alive here is
/// what lets a test re-draw and re-commit the same window afterwards.
pub struct Toplevel {
    /// The underlying `wl_surface`.
    pub surface: WlSurface,
    /// Its `xdg_surface` (the role-agnostic half: geometry, `ack_configure`).
    pub xdg_surface: XdgSurface,
    /// Its `xdg_toplevel` (title, app_id, destroy).
    pub toplevel: XdgToplevel,
    /// The `wl_shm` buffer holding the window's pixels.
    pub buffer: WlBuffer,
    /// The pool backing that buffer, writable for a re-draw.
    pub pool: ShmPool,
}

/// A scripted Wayland client on an in-process connection to a [`ProtocolHost`].
///
/// [`ProtocolHost`]: parhelion_core::protocol::ProtocolHost
pub struct ScriptedClient {
    /// The connection. Kept alive for the client's duration (dropping it
    /// disconnects) and used to [`flush`](Self::flush) requests without a
    /// round-trip.
    conn: Connection,
    /// The client's event queue.
    queue: EventQueue<App>,
    /// Client-side app state (records frame-callback timestamps).
    app: App,
    /// The bound `wl_compositor`, present iff the global was advertised.
    compositor: WlCompositor,
    /// The bound `wl_shm`, present iff the global was advertised (T3).
    shm: WlShm,
    /// The bound `xdg_wm_base`, present iff the global was advertised (T5).
    wm_base: XdgWmBase,
    // The seat objects are held, not read: they exist so the compositor has
    // somewhere to deliver keyboard and pointer events (an unbound seat receives
    // nothing), and the rig records those events through the `Dispatch` impls
    // rather than through these handles. Held rather than dropped so the client's
    // object lifetime matches a real application's.
    /// The bound `wl_output` (T7). Held so the compositor keeps sending this
    /// client output state, and used to ask for the matching `xdg_output`.
    output: WlOutput,
    /// The `xdg_output` for [`output`](Self::output), bound lazily by the tests
    /// that ask about logical geometry.
    xdg_output: Option<ZxdgOutputV1>,
    /// The registry's global list, kept so late binds (like `xdg_output`'s
    /// manager) do not need a second registry round-trip.
    globals: wayland_client::globals::GlobalList,
    /// The clipboard contents this client has published, answered when the
    /// compositor forwards a `send` request from whoever is pasting.
    pending_clipboard: Option<(WlDataSource, Vec<u8>)>,
    /// The bound `wl_data_device_manager` and this client's device on the seat
    /// (T7b) — how the clipboard reaches a client.
    data_device_manager: WlDataDeviceManager,
    #[allow(dead_code)]
    data_device: WlDataDevice,
    /// The bound `wl_seat` (T6). Held so the compositor keeps delivering this
    /// client's input; the device objects taken from it do the talking.
    #[allow(dead_code)]
    seat: WlSeat,
    /// This client's `wl_keyboard`, obtained at connect so no test can forget to
    /// ask for one before asserting on focus.
    #[allow(dead_code)]
    keyboard: WlKeyboard,
    /// This client's `wl_pointer`, likewise.
    #[allow(dead_code)]
    pointer: WlPointer,
}

impl ScriptedClient {
    /// Connect over `stream` (the client end of a socketpair whose server end
    /// was handed to [`ProtocolHost::add_client`]), initialise the registry,
    /// and bind `wl_compositor`. Panics if the compositor global is absent —
    /// its presence is part of what the rig asserts (wire behaviour).
    ///
    /// [`ProtocolHost::add_client`]: parhelion_core::protocol::ProtocolHost::add_client
    pub fn connect(stream: UnixStream) -> Self {
        let conn = Connection::from_socket(stream).expect("client connection");
        let (globals, queue) = registry_queue_init::<App>(&conn).expect("registry init");
        let qh = queue.handle();
        // Binding proves the server advertised wl_compositor (versions 1..=4).
        let compositor: WlCompositor = globals
            .bind(&qh, 1..=4, ())
            .expect("wl_compositor global advertised");
        // Bind wl_shm (T3). Presence is part of what the rig asserts.
        let shm: WlShm = globals
            .bind(&qh, 1..=1, ())
            .expect("wl_shm global advertised");
        // Bind xdg_wm_base (T5). Version 1 is all the rig needs; the server
        // advertises 6.
        let wm_base: XdgWmBase = globals
            .bind(&qh, 1..=6, ())
            .expect("xdg_wm_base global advertised");
        // Bind wl_seat and take both capabilities (T6). Presence of the global is
        // part of what the rig asserts; taking keyboard and pointer here means a
        // test never has to remember to.
        let seat: WlSeat = globals
            .bind(&qh, 1..=9, ())
            .expect("wl_seat global advertised");
        let keyboard = seat.get_keyboard(&qh, ());
        let pointer = seat.get_pointer(&qh, ());
        // Bind wl_output (T7). A real client binds it before it draws, to learn
        // the screen's size and scale; the rig does the same so its surfaces
        // receive enter/leave.
        let output: WlOutput = globals
            .bind(&qh, 1..=4, ())
            .expect("wl_output global advertised");
        // Bind the data device (T7b): a client with no data device never hears
        // about the clipboard, so every rig client takes one, as real clients do.
        let data_device_manager: WlDataDeviceManager = globals
            .bind(&qh, 1..=3, ())
            .expect("wl_data_device_manager global advertised");
        let data_device = data_device_manager.get_data_device(&seat, &qh, ());
        ScriptedClient {
            conn,
            queue,
            app: App::default(),
            compositor,
            shm,
            wm_base,
            output,
            xdg_output: None,
            globals,
            data_device_manager,
            data_device,
            pending_clipboard: None,
            seat,
            keyboard,
            pointer,
        }
    }

    /// Create a `wl_surface` and return its client-side proxy.
    pub fn create_surface(&mut self) -> WlSurface {
        let qh = self.queue.handle();
        self.compositor.create_surface(&qh, ())
    }

    /// Request a frame callback on `surface` (`wl_surface.frame`) and return its
    /// client-side proxy. The callback is *pending* until the commit that
    /// carries it; its `done` (recorded in [`frame_dones`](Self::frame_dones))
    /// fires when the compositor presents that commit.
    pub fn frame(&mut self, surface: &WlSurface) -> WlCallback {
        let qh = self.queue.handle();
        surface.frame(&qh, ())
    }

    /// Commit a surface.
    pub fn commit(&self, surface: &WlSurface) {
        surface.commit();
    }

    /// Destroy a surface (`wl_surface.destroy`).
    pub fn destroy(&self, surface: WlSurface) {
        surface.destroy();
    }

    /// Flush what fits, and treat a full socket as success.
    ///
    /// This is what a flooding client actually experiences once backpressure
    /// bites: the compositor stops reading, the kernel buffer fills, and the
    /// client's own writes start returning `WouldBlock`. That is the mechanism
    /// working, not a failure — so a test that floods deliberately uses this
    /// rather than [`flush`](Self::flush), which panics on it.
    pub fn flush_best_effort(&self) {
        match self.conn.flush() {
            Ok(()) => {}
            Err(wayland_client::backend::WaylandError::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("client flush failed for a reason other than a full socket: {e}"),
        }
    }

    /// Flush buffered requests to the socket **without** waiting for the server
    /// (no round-trip). Used to pile requests onto a deliberately-throttled
    /// socket in the backpressure test, where a round-trip would block forever
    /// (the server has stopped reading that client).
    pub fn flush(&self) {
        self.conn.flush().expect("client flush");
    }

    /// Flush pending requests and block until the server has processed them
    /// (the round-trip's internal `wl_display.sync` returns only afterwards).
    /// This also dispatches any events already received — including
    /// `wl_callback.done` — into [`frame_dones`](Self::frame_dones).
    pub fn roundtrip(&mut self) {
        self.queue
            .roundtrip(&mut self.app)
            .expect("client roundtrip");
    }

    /// The timestamps of every `wl_callback.done` this client has received so
    /// far (in arrival order). Populated as round-trips dispatch the events.
    pub fn frame_dones(&self) -> &[u32] {
        &self.app.frame_dones
    }

    /// Number of `wl_buffer.release` events received so far — the T3 evidence that
    /// the compositor released after copying (populated as round-trips dispatch).
    pub fn buffer_releases(&self) -> u32 {
        self.app.buffer_releases
    }

    /// Create a `wl_shm` pool backed by an unnamed temp file of `size` bytes. The
    /// returned [`ShmPool`] owns the file so the test can write pixels into it
    /// (and rewrite it between commits — the single-buffer client pattern).
    pub fn create_pool(&mut self, size: usize) -> ShmPool {
        let file = tempfile::tempfile().expect("create shm temp file");
        file.set_len(size as u64).expect("size shm pool");
        let qh = self.queue.handle();
        let pool = self.shm.create_pool(file.as_fd(), size as i32, &qh, ());
        ShmPool { pool, file }
    }

    /// Create a `wl_buffer` from `pool` covering the whole pool at offset 0, with
    /// a tightly-packed stride (`width * 4`) in `format`.
    pub fn create_buffer(
        &mut self,
        pool: &ShmPool,
        width: i32,
        height: i32,
        format: ShmFormat,
    ) -> WlBuffer {
        let qh = self.queue.handle();
        pool.pool
            .create_buffer(0, width, height, width * 4, format.to_wl(), &qh, ())
    }

    /// Create a `wl_buffer` with **explicit** offset and stride, including values
    /// that do not fit the pool. The conformance tests need this: a compositor
    /// must reject impossible geometry rather than read out of bounds, and the
    /// only way to check that is to ask for it.
    pub fn create_buffer_raw(
        &mut self,
        pool: &ShmPool,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format: ShmFormat,
    ) -> WlBuffer {
        let qh = self.queue.handle();
        pool.pool
            .create_buffer(offset, width, height, stride, format.to_wl(), &qh, ())
    }

    /// Attach `buffer` to `surface` at offset (0, 0). Double-buffered: applied on
    /// the next [`commit`](Self::commit).
    pub fn attach(&self, surface: &WlSurface, buffer: &WlBuffer) {
        surface.attach(Some(buffer), 0, 0);
    }

    /// Attach a null buffer to `surface` — unmaps its content on the next commit.
    pub fn attach_null(&self, surface: &WlSurface) {
        surface.attach(None, 0, 0);
    }

    /// Post surface damage (`wl_surface.damage`, surface coordinates) — the region
    /// the client says changed, applied at the next commit.
    pub fn damage(&self, surface: &WlSurface, x: i32, y: i32, w: i32, h: i32) {
        surface.damage(x, y, w, h);
    }

    // ----- xdg-shell (T5) ---------------------------------------------------
    //
    // The pieces of the toplevel dance, exposed individually *and* as the
    // `map_toplevel` helper below. Individually, because the conformance tests
    // exist to break the dance in specific ways (commit a buffer before acking,
    // ack a serial that was never sent, take a second role); as a helper, because
    // every other test just wants a mapped window and should read that way.

    /// Give `surface` an `xdg_surface` (`xdg_wm_base.get_xdg_surface`).
    pub fn create_xdg_surface(&mut self, surface: &WlSurface) -> XdgSurface {
        let qh = self.queue.handle();
        self.wm_base.get_xdg_surface(surface, &qh, ())
    }

    /// Give an `xdg_surface` the toplevel role (`xdg_surface.get_toplevel`).
    /// Calling this twice on one surface is the double-role protocol error the
    /// conformance test provokes.
    pub fn get_toplevel(&mut self, xdg_surface: &XdgSurface) -> XdgToplevel {
        let qh = self.queue.handle();
        xdg_surface.get_toplevel(&qh, ())
    }

    /// Ask for a subsurface (`wl_subcompositor.get_subsurface`).
    ///
    /// Parhelion advertises `wl_subcompositor` because clients refuse to start
    /// without it, and refuses this request with a protocol error because the
    /// scene does not composite subsurfaces yet (M2 T0's tripwire; the real
    /// implementation is M2 T7). The rig has this so a test can prove the refusal
    /// is *loud*.
    pub fn get_subsurface(&mut self, surface: &WlSurface, parent: &WlSurface) -> WlSubsurface {
        let qh = self.queue.handle();
        let subcompositor: WlSubcompositor = self
            .globals
            .bind(&qh, 1..=1, ())
            .expect("wl_subcompositor global advertised");
        subcompositor.get_subsurface(surface, parent, &qh, ())
    }

    /// Create an `xdg_positioner` (`xdg_wm_base.create_positioner`). Needed only
    /// to reach `get_popup`; the rig never configures it, because the one test
    /// that uses it errors on the role before the positioner is ever read.
    pub fn create_positioner(&mut self) -> XdgPositioner {
        let qh = self.queue.handle();
        self.wm_base.create_positioner(&qh, ())
    }

    /// Give an `xdg_surface` the **popup** role (`xdg_surface.get_popup`). Used to
    /// ask a surface that is already a toplevel for a second, different role —
    /// the protocol error the conformance test pins. (Popups themselves are out
    /// of scope for M1; Parhelion dismisses any it is asked for.)
    pub fn get_popup(
        &mut self,
        xdg_surface: &XdgSurface,
        parent: &XdgSurface,
        positioner: &XdgPositioner,
    ) -> XdgPopup {
        let qh = self.queue.handle();
        xdg_surface.get_popup(Some(parent), positioner, &qh, ())
    }

    /// Acknowledge a configure serial (`xdg_surface.ack_configure`). A serial the
    /// compositor never sent is a protocol error — deliberately reachable, since
    /// that is one of the cases under test.
    pub fn ack_configure(&self, xdg_surface: &XdgSurface, serial: u32) {
        xdg_surface.ack_configure(serial);
    }

    /// Block (by round-tripping) until an unconsumed `xdg_surface.configure`
    /// arrives, then take its serial. Deterministic: it waits on a definite
    /// condition — the configure the compositor owes the initial commit — with a
    /// generous round-trip budget as a loud-failure net, never a sleep.
    pub fn wait_for_configure(&mut self) -> u32 {
        for _ in 0..1000 {
            if let Some(serial) = self.app.xdg_configures.pop_front() {
                return serial;
            }
            self.roundtrip();
        }
        panic!("no xdg_surface.configure within the round-trip budget");
    }

    /// Set a toplevel's title (`xdg_toplevel.set_title`).
    pub fn set_title(&self, toplevel: &XdgToplevel, title: &str) {
        toplevel.set_title(title.to_string());
    }

    /// Set a toplevel's app id (`xdg_toplevel.set_app_id`).
    pub fn set_app_id(&self, toplevel: &XdgToplevel, app_id: &str) {
        toplevel.set_app_id(app_id.to_string());
    }

    /// Perform the **whole** toplevel mapping dance and return the mapped window:
    ///
    /// ```text
    /// create surface → get_xdg_surface → get_toplevel
    ///   → commit (no buffer: the initial commit)
    ///   → receive configure → ack_configure
    ///   → draw → attach → commit          ← this commit maps it
    /// ```
    ///
    /// `draw` runs between the configure and the mapping commit, which is exactly
    /// where a real client paints: it now knows what the compositor suggested. The
    /// pool it receives is `width * height * 4` bytes.
    pub fn map_toplevel_with(
        &mut self,
        width: i32,
        height: i32,
        format: ShmFormat,
        draw: impl FnOnce(&mut ShmPool),
    ) -> Toplevel {
        let surface = self.create_surface();
        let xdg_surface = self.create_xdg_surface(&surface);
        let toplevel = self.get_toplevel(&xdg_surface);

        // The initial commit carries no buffer; the compositor answers with a
        // configure, which the client must ack before any buffer may follow.
        self.commit(&surface);
        let serial = self.wait_for_configure();
        self.ack_configure(&xdg_surface, serial);

        let mut pool = self.create_pool((width * height * 4) as usize);
        draw(&mut pool);
        let buffer = self.create_buffer(&pool, width, height, format);
        self.attach(&surface, &buffer);
        self.commit(&surface);
        self.roundtrip();

        Toplevel {
            surface,
            xdg_surface,
            toplevel,
            buffer,
            pool,
        }
    }

    /// [`map_toplevel_with`](Self::map_toplevel_with) with the buffer contents
    /// given up front — the common case.
    pub fn map_toplevel(
        &mut self,
        width: i32,
        height: i32,
        format: ShmFormat,
        pixels: &[u8],
    ) -> Toplevel {
        self.map_toplevel_with(width, height, format, |pool| pool.write(pixels))
    }

    /// Sizes carried by the `xdg_toplevel.configure` events received so far.
    /// Parhelion's C10 fallback sends `(0, 0)` — "you choose".
    pub fn toplevel_configures(&self) -> &[(i32, i32)] {
        &self.app.toplevel_configures
    }

    /// Number of `xdg_wm_base.ping` events received (each answered with `pong`).
    pub fn pings_received(&self) -> u32 {
        self.app.pings
    }

    // ----- Seat / input observation (T6) ------------------------------------

    /// Every seat event received so far, in arrival order. Ordering is
    /// load-bearing (`enter` before input, `leave` before the next `enter`), so
    /// tests assert on slices of this list rather than on per-kind counters.
    pub fn input_events(&self) -> &[SeatEvent] {
        &self.app.input_events
    }

    /// Drop all recorded seat events — lets a test ignore the setup traffic
    /// (focus arriving as windows map) and assert only on what it then provokes.
    pub fn clear_input_events(&mut self) {
        self.app.input_events.clear();
    }

    /// The xkb keymap the compositor sent, as text (`None` until it arrives).
    pub fn keymap(&self) -> Option<&str> {
        self.app.keymap.as_deref()
    }

    /// The `wl_seat` capability bitfield the compositor advertised.
    pub fn seat_capabilities(&self) -> u32 {
        self.app.seat_capabilities
    }

    /// The client-side object id of a surface — how [`SeatEvent`]s name the
    /// surface an event was delivered to.
    pub fn surface_id(&self, surface: &WlSurface) -> u32 {
        surface.id().protocol_id()
    }

    /// Pump round-trips until at least `n` seat events have arrived, or fail
    /// loudly. Deterministic: the events are already in flight when this is
    /// called (the compositor sent them before the sync reply), so this waits on
    /// a definite condition rather than a timer.
    pub fn pump_until_input_events(&mut self, n: usize) {
        for _ in 0..1000 {
            if self.app.input_events.len() >= n {
                return;
            }
            self.roundtrip();
        }
        panic!(
            "expected at least {n} seat events, got {}: {:?}",
            self.app.input_events.len(),
            self.app.input_events
        );
    }

    // ----- Output observation (T7) ------------------------------------------

    /// The output mode the compositor advertised: `(width, height, refresh_mHz)`.
    pub fn output_mode(&self) -> Option<(i32, i32, i32)> {
        self.app.output_mode
    }

    /// The output scale factor advertised (`wl_output.scale`).
    pub fn output_scale(&self) -> i32 {
        self.app.output_scale
    }

    /// How many `wl_output.done` events have closed a batch of output state.
    pub fn output_done_count(&self) -> u32 {
        self.app.output_done
    }

    /// `wl_surface.enter`/`leave` in arrival order, as
    /// `(client-side surface id, is_enter)`.
    pub fn surface_outputs(&self) -> &[(u32, bool)] {
        &self.app.surface_outputs
    }

    /// Bind `xdg_output` for this client's output and pump until its logical
    /// geometry arrives, then return the logical size.
    pub fn xdg_output_logical_size(&mut self) -> (i32, i32) {
        self.ensure_xdg_output();
        self.app
            .xdg_logical_size
            .expect("xdg_output reported a logical size")
    }

    /// As [`xdg_output_logical_size`](Self::xdg_output_logical_size), for the
    /// logical position.
    pub fn xdg_output_logical_position(&mut self) -> (i32, i32) {
        self.ensure_xdg_output();
        self.app
            .xdg_logical_position
            .expect("xdg_output reported a logical position")
    }

    /// Bind the manager (once) and wait for the geometry batch.
    fn ensure_xdg_output(&mut self) {
        if self.xdg_output.is_none() {
            let qh = self.queue.handle();
            let manager: ZxdgOutputManagerV1 = self
                .globals
                .bind(&qh, 1..=3, ())
                .expect("zxdg_output_manager_v1 global advertised");
            self.xdg_output = Some(manager.get_xdg_output(&self.output, &qh, ()));
        }
        for _ in 0..1000 {
            if self.app.xdg_logical_size.is_some() && self.app.xdg_logical_position.is_some() {
                return;
            }
            self.roundtrip();
        }
        panic!("xdg_output geometry never arrived");
    }

    /// Every global the compositor advertises, as `(interface, version)`.
    ///
    /// Used to assert what is *not* there as well as what is: a global we
    /// advertise but do not honour is a standing lie, so "the registry no longer
    /// lists `wl_subcompositor`" is a property worth pinning.
    pub fn advertised_globals(&self) -> Vec<(String, u32)> {
        self.globals
            .contents()
            .clone_list()
            .into_iter()
            .map(|g| (g.interface, g.version))
            .collect()
    }

    // ----- Clipboard (T7b) ---------------------------------------------------

    /// Offer `contents` on the clipboard under one mime type.
    ///
    /// Returns the source so the test can watch it be cancelled. The serial is
    /// the one the protocol requires: a client may only set the selection using a
    /// serial from an input event it actually received, which is the protocol's
    /// own answer to "who may overwrite the clipboard" — and the reason an
    /// unfocused client cannot.
    pub fn set_clipboard(&mut self, mime: &str, contents: &[u8]) -> WlDataSource {
        let qh = self.queue.handle();
        let source = self.data_device_manager.create_data_source(&qh, ());
        source.offer(mime.to_string());
        self.pending_clipboard = Some((source.clone(), contents.to_vec()));
        let serial = self.last_input_serial();
        self.data_device.set_selection(Some(&source), serial);
        source
    }

    /// Start a drag from `window`'s surface, offering one mime type.
    ///
    /// The caller must first give this client a **pointer grab** — a button press
    /// over the window — because the protocol only permits a drag in response to
    /// one, and a compositor must deny anything else. `press_pointer_on` does
    /// that. Parhelion's v1 policy then cancels the drag immediately (see the
    /// compositor's DnD handler), which is what the corresponding test asserts.
    pub fn start_drag(&mut self, window: &Toplevel) -> WlDataSource {
        let qh = self.queue.handle();
        let source = self.data_device_manager.create_data_source(&qh, ());
        source.offer("text/plain".to_string());
        let serial = self.last_input_serial();
        self.data_device
            .start_drag(Some(&source), &window.surface, None, serial);
        source
    }

    /// The serial of the most recent input event this client received — what
    /// `set_selection` must be given. Zero if it has never had input, which is
    /// exactly the case an unfocused client is in.
    pub fn last_input_serial(&self) -> u32 {
        self.app
            .input_events
            .iter()
            .rev()
            .find_map(|e| match e {
                SeatEvent::Key { serial, .. } | SeatEvent::PointerButton { serial, .. } => {
                    Some(*serial)
                }
                _ => None,
            })
            .unwrap_or(0)
    }

    /// The mime types the current clipboard offer advertises (empty if there is
    /// no offer).
    pub fn clipboard_mimes(&self) -> &[String] {
        &self.app.offer_mimes
    }

    /// Whether this client currently holds a clipboard offer.
    pub fn has_clipboard_offer(&self) -> bool {
        self.app.selection_offer.is_some()
    }

    /// How many `selection` events have arrived — for waiting on "the clipboard
    /// changed" without a timer.
    pub fn selection_event_count(&self) -> u32 {
        self.app.selection_events
    }

    /// How many of this client's data sources have been cancelled.
    pub fn source_cancelled_count(&self) -> u32 {
        self.app.source_cancelled
    }

    /// Read the clipboard: ask the offer for `mime`, answer any `send` the
    /// compositor forwards to *this* client's own source (a client can be both
    /// ends), and return the bytes that came back through the pipe.
    ///
    /// This is the real transfer — a pipe, a write from the source client, a read
    /// here — not a compositor-side copy. The compositor never sees the bytes,
    /// which is precisely the design: it brokers the introduction and gets out of
    /// the way.
    pub fn read_clipboard(&mut self, mime: &str, peers: &mut [&mut ScriptedClient]) -> Vec<u8> {
        let offer = self
            .app
            .selection_offer
            .clone()
            .expect("this client holds a clipboard offer");
        let (read_fd, write_fd) = std::io::pipe().expect("clipboard pipe");
        offer.receive(mime.to_string(), write_fd.as_fd());
        self.flush();
        drop(write_fd); // only the source keeps a writer, or the read never ends

        // Give every client a chance to notice the `send` and answer it.
        for _ in 0..1000 {
            self.roundtrip();
            for peer in peers.iter_mut() {
                peer.answer_clipboard_sends();
            }
            self.answer_clipboard_sends();
            if let Some(bytes) = try_read_to_end(&read_fd) {
                return bytes;
            }
        }
        panic!("no clipboard bytes arrived through the pipe");
    }

    /// Answer any pending `send` request on this client's data source by writing
    /// the contents it published, then closing the pipe (the reader sees EOF).
    pub fn answer_clipboard_sends(&mut self) {
        self.roundtrip();
        let pending = std::mem::take(&mut self.app.send_requests);
        for (_mime, fd) in pending {
            if let Some((_, contents)) = &self.pending_clipboard {
                let mut file = File::from(fd);
                let _ = file.write_all(contents);
                // Dropping `file` closes the write end: EOF for the reader.
            }
        }
    }

    // ----- Protocol-error observation (T5) ----------------------------------

    /// The protocol error the server posted to this client, if any — pumping
    /// round-trips until one arrives or the budget runs out.
    ///
    /// A protocol error puts the connection permanently in an error state and the
    /// server drops the client, so the *first* failing round-trip is where it
    /// surfaces; after that the error is readable from the connection forever.
    /// That makes this deterministic: no sleeps, and no ambiguity between "not
    /// yet" and "never".
    pub fn protocol_error(&mut self) -> Option<RigProtocolError> {
        for _ in 0..1000 {
            if let Some(err) = self.conn.protocol_error() {
                return Some(RigProtocolError {
                    code: err.code,
                    interface: err.object_interface,
                    message: err.message,
                });
            }
            // Once the connection is in an error state every dispatch fails; stop
            // pumping and read the error out below.
            if self.queue.roundtrip(&mut self.app).is_err() {
                break;
            }
        }
        self.conn.protocol_error().map(|err| RigProtocolError {
            code: err.code,
            interface: err.object_interface,
            message: err.message,
        })
    }

    /// Like [`protocol_error`](Self::protocol_error) but fails the test if the
    /// server did *not* post an error — the assertion form, so a silently-accepted
    /// violation shows up as a failure rather than a skipped check.
    pub fn expect_protocol_error(&mut self) -> RigProtocolError {
        self.protocol_error()
            .expect("expected the server to post a protocol error, but the client is still healthy")
    }
}

/// The two `wl_shm` formats the protocol mandates (and Parhelion supports in T3).
/// A rig-level enum so tests need not depend on `wayland-client` directly.
#[derive(Clone, Copy, Debug)]
pub enum ShmFormat {
    /// 32-bit, alpha channel honoured — the compositor blends it.
    Argb8888,
    /// 32-bit, alpha ignored (treated as opaque) — the compositor overwrites.
    Xrgb8888,
}

impl ShmFormat {
    /// Map to the wire-level `wl_shm` format.
    fn to_wl(self) -> Format {
        match self {
            ShmFormat::Argb8888 => Format::Argb8888,
            ShmFormat::Xrgb8888 => Format::Xrgb8888,
        }
    }
}

/// A `wl_shm` pool the scripted client can draw into. Owns the backing temp file
/// so the client can (re)write pixels; the compositor mmaps the same file.
pub struct ShmPool {
    /// The bound `wl_shm_pool`.
    pool: WlShmPool,
    /// The pool's backing file (shared with the compositor's mmap).
    file: File,
}

impl ShmPool {
    /// Write `bytes` at the start of the pool (offset 0). Used to draw a buffer's
    /// contents, and to *re*-draw the same buffer between commits (single-buffer
    /// client pattern) — the compositor sees the new bytes on the next commit.
    pub fn write(&mut self, bytes: &[u8]) {
        self.file.seek(SeekFrom::Start(0)).expect("seek shm pool");
        self.file.write_all(bytes).expect("write shm pool");
        self.file.flush().expect("flush shm pool");
    }
}
