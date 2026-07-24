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

use std::os::unix::net::UnixStream;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};

/// Client-side dispatch state. The rig's client sends requests and, for T2, must
/// observe one kind of event: `wl_callback.done` from `wl_surface.frame`
/// callbacks. Every other global's handler stays empty.
#[derive(Default)]
struct App {
    /// Timestamps carried by every `wl_callback.done` received, in arrival order
    /// — the reverse-direction evidence a test asserts on.
    frame_dones: Vec<u32>,
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
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wayland_client::protocol::wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
        ScriptedClient {
            conn,
            queue,
            app: App::default(),
            compositor,
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
}
