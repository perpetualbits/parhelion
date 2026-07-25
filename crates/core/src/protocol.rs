//! `ProtocolHost` — the Wayland protocol frontend, at `shards = 1`.
//!
//! Governing design: `docs/CORE-BOUNDARY.md` §3 (C3: protocol machinery is
//! in-core) and §7 (T-proto[n] owns client sockets + protocol object state and
//! publishes changes to the scene by message). Governing decision: the three
//! entries under "2026-07-24 — Smithay threading fit" in the decision log —
//! this module implements their four interface requirements. It is governed by
//! CORE-BOUNDARY §3/§7 and, for the scene edge it now feeds, by
//! `docs/scene_graph_v1.md`.
//!
//! # The four requirements, and where each lives
//!
//! 1. **Client→shard assignment at accept time, inside `ProtocolHost`.**
//!    [`ProtocolHost::add_client`] is the single seam: an external
//!    `ListeningSocket::accept` loop and the test rig both feed it a
//!    `UnixStream`, and it routes the stream to a shard. At `shards = 1` there
//!    is one shard (one dispatch thread, one `Display`); nothing *outside* this
//!    module names a `Display` or assumes the count. Growing to N shards is an
//!    `add_client` change (pick a shard), not an architectural one.
//! 2. **Dispatch thread owns `Display` + a thin protocol-only [`State`]; only
//!    `Send` tokens cross out.** The thread (see [`run_dispatch`]) owns the
//!    `Display`; everything it tells the world is a [`ProtocolEvent`] carrying
//!    core-assigned [`SurfaceId`]/[`ClientKey`], never a borrowed `WlSurface`.
//! 3. **The receiving side is the scene owner.** Where M0 published to a minimal
//!    in-`ProtocolHost` ledger, M1 publishes to the real canonical scene through
//!    a [`SceneHandle`] (`crate::scene`): the ledger was absorbed
//!    (`docs/scene_graph_v1.md`). The edge is still one-directional and async —
//!    the dispatch thread calls [`SceneHandle::emit`], which never blocks on a
//!    reply (I-3).
//! 4. **Globals advertised identically per shard.** `wl_compositor` (+ the
//!    subcompositor machinery Smithay's delegate brings) is created once per
//!    `Display`; with one shard that is one advertisement, and the code path is
//!    shard-count-agnostic.
//!
//! # Mechanism
//!
//! Dispatch runs on `calloop` — the production substrate and Smithay's
//! (`docs/smithay_threading_spike.md`: mechanism, not threading model). The
//! `Display` fd is a `Generic` source; a `calloop` channel carries control
//! messages (admit client / shut down) into the thread; a `calloop` **ping**
//! source carries the render side's "frame presented" notice (the reverse edge).
//!
//! # The reverse edge (T2), and flush ownership
//!
//! M1-T1 opened one direction only: client → scene. T2 opens client ← render.
//! The render loop ([`crate::render::RenderLoop`]) never touches a Wayland
//! object; it only *enqueues* a "frame presented at `t`" notice through a
//! [`FramePresenter`] (an atomic timestamp + a `calloop` ping — wait-free, I-1).
//! The dispatch thread — the sole owner of every protocol object (§7) — wakes on
//! the ping and turns the notice into `wl_surface.frame` → `wl_callback.done`
//! sends ([`present`]). **One thread touches Wayland objects, period.**
//!
//! Flushing is likewise owned in exactly one place: the loop body flushes once
//! per iteration, *after* every source has run (client replies dispatched,
//! `done` events enqueued). Every callback here only *enqueues*; nothing else
//! flushes. See the single `flush_clients` call in [`run_dispatch`].
//!
//! # Backpressure (T2, the I-10 fairness rider)
//!
//! Both queues that now couple the two threads are bounded. The render→dispatch
//! notice coalesces to a single slot (last timestamp wins; see [`FramePresenter`]).
//! Per client, the pending frame-callback backlog is capped at
//! [`MAX_PENDING_FRAME_CALLBACKS`]: a client over the cap has its socket left
//! unread (`pump_display` dispatches per client and skips it) until a tick
//! drains its callbacks — bounded memory, no dropped messages, no stall for
//! shard-mates. Policy lives in `docs/scene_graph_v1.md` §8.
//!
//! # xdg-shell and the mapping rule (T5)
//!
//! `xdg_wm_base` / `xdg_surface` / `xdg_toplevel` ride
//! `smithay::wayland::shell::xdg` (again: frontend only, no renderer type). The
//! lifecycle this module enforces is the protocol's, strictly:
//!
//! 1. **Initial commit, no buffer** → the compositor answers with a `configure`
//!    (0×0 — the client picks its own size; no states in v1).
//! 2. **The client must `ack_configure` before committing a buffer.** A buffer on
//!    an unacked surface is a protocol error ([`State::commit`]).
//! 3. **First buffer commit maps the toplevel** — it takes its C10 placement
//!    ([`CASCADE_STEP_X`]) and becomes visible scene content.
//! 4. **Null attach or `xdg_toplevel.destroy` unmaps it** — the scene node loses
//!    its source (and, on destroy, its role), with structural damage.
//!
//! The load-bearing consequence, and the reason this task is a migration:
//! **only mapped toplevels (and core-injected C10/harness content) are ever
//! displayed.** A bare committed `wl_surface` is live in the scene and invisible,
//! per Wayland (`crate::scene::NodeRole`, `docs/scene_graph_v1.md` §10).
//!
//! # Protocol scope (M0/M1 T1–T5)
//!
//! `wl_compositor` (surface create / commit / destroy, `wl_surface.frame`
//! callbacks — T2), `wl_shm` (T3: shared-memory buffers, copied at commit into
//! a scene-side pixel block and released immediately), and `xdg_wm_base` (T5:
//! toplevels, above), via `smithay::wayland::{compositor, shm, shell::xdg}` (the
//! frontend layers the decision points at; Smithay's renderer layer is never
//! touched). Popups, input (T6), and decorations are later.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use smithay::reexports::calloop::{
    channel::{channel as calloop_channel, Channel, Event as ChannelEvent, Sender as CalloopSender},
    generic::Generic,
    ping::{make_ping, Ping, PingSource},
    EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, ObjectId,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_callback::WlCallback;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, Display, DisplayHandle, ListeningSocket, Resource,
};
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::input::keyboard::{FilterResult, KeyboardHandle, Keycode, XkbConfig};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent, PointerHandle,
};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_role, is_sync_subsurface, with_states, with_surface_tree_upward, BufferAssignment,
    CompositorClientState, CompositorHandler, CompositorState, Damage, SubsurfaceCachedState,
    SurfaceAttributes, TraversalAction, SUBSURFACE_ROLE,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::selection::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
    ServerDndGrabHandler,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ShellClient, ToplevelSurface, XdgShellHandler, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{with_buffer_contents, BufferAccessError, ShmHandler, ShmState};

use crate::input::{FocusMap, InputEvent};
use crate::scene::{
    ClientKey, ContentDamage, NodeRole, PixelBuffer, ProtocolEvent, Rect, SceneHandle, SurfaceId,
    SurfaceUpdate, TextureSource, ToplevelRole, MAX_SUBSURFACE_DEPTH,
};

/// Per-client cap on **pending** (committed but not-yet-fired) `wl_surface.frame`
/// callbacks — the M1 backpressure bound and invariant **I-10**'s fairness rider
/// (`docs/scene_graph_v1.md` §8, "Backpressure policy").
///
/// This is the one queue a client can grow *without bound* on its own: frame
/// callbacks only drain on a render tick (the reverse edge below), and the tick
/// is not under the client's control, so a client that commits frame requests in
/// a tight loop piles them up until something stops it. That "something" is this
/// bound: once a client's pending-callback backlog reaches it, the dispatch loop
/// stops reading that client's socket (`pump_display`) until a tick drains the
/// callbacks — never dropping messages, never stalling shard-mates.
///
/// The value is deliberately generous. A well-behaved client keeps ≤1–2
/// callbacks in flight (it waits for `done` before drawing again), so 64 is
/// ~1 second of unacknowledged frames at 60 Hz — orders of magnitude past any
/// honest need, yet a hard ceiling on a flooder's in-core footprint.
pub const MAX_PENDING_FRAME_CALLBACKS: usize = 64;

// ==========================================================================
// C10 fallback placement — LOUDLY TEMPORARY.
//
// `CORE-BOUNDARY.md` C10 puts "default window placement" in the core precisely
// so the compositor stays usable with every server dead. It is *not* the core's
// job to decide where windows go: that is policy (§4 rule 4), and policy lives
// in the reference policy daemon S1, which arrives in **M4**. Until then a
// toplevel is placed by this cascade and nothing else.
//
// The one property that matters today is **determinism** — goldens depend on
// the nth toplevel of a run landing in exactly the same place every time — so
// the cascade is a pure function of the toplevel's creation index, with no
// clock, no randomness, and no dependence on what else is on screen.
// ==========================================================================

/// X of the first toplevel's top-left corner. Zero: the first window sits at the
/// output origin, which is also where the pre-T5 raw-commit path put content.
pub const CASCADE_ORIGIN_X: i32 = 0;
/// Y of the first toplevel's top-left corner (see [`CASCADE_ORIGIN_X`]).
pub const CASCADE_ORIGIN_Y: i32 = 0;
/// Horizontal offset added per subsequent toplevel.
pub const CASCADE_STEP_X: i32 = 32;
/// Vertical offset added per subsequent toplevel.
pub const CASCADE_STEP_Y: i32 = 32;
/// Number of steps before the cascade returns to the origin. The core does not
/// know the output size (outputs arrive with the DRM backend in M2), so the
/// cascade cannot clamp to a screen; wrapping is what keeps it from walking off
/// into the far corner of an unbounded plane.
pub const CASCADE_WRAP: u64 = 8;

/// The backlog at which a throttled client's socket is polled again (M2 T0).
///
/// Hysteresis: a client is throttled at [`MAX_PENDING_FRAME_CALLBACKS`] (64) and
/// resumed only once it has drained to a **quarter** of that. Re-arming at the
/// same mark it was throttled at would make a steady flooder oscillate — disabled
/// and enabled on alternate ticks, one syscall each way, for as long as it kept
/// flooding. A gap of 48 callbacks means a client that is merely *busy* resumes
/// after one render tick drains it, while a client that is genuinely flooding
/// stays parked until it stops.
pub const RESUME_PENDING_FRAME_CALLBACKS: usize = MAX_PENDING_FRAME_CALLBACKS / 4;

// ==========================================================================
// Protocol error codes we post ourselves (M2 T0).
// ==========================================================================

/// `wl_display.error.implementation` — "the compositor cannot implement this
/// request". The protocol's own way for a server to admit a limitation rather
/// than blame the client, and what libwayland posts from
/// `wl_client_post_implementation_error`. Used by the subsurface tripwire.
pub const WL_DISPLAY_ERROR_IMPLEMENTATION: u32 = 3;

// ==========================================================================
// Seat constants (T6).
// ==========================================================================

/// The seat's name, as advertised to clients. One seat is all M1 models
/// (`CORE-BOUNDARY.md` C2); multi-seat is not a milestone concern.
pub const SEAT_NAME: &str = "seat0";

/// Delay before key repeat starts, in ms — advertised via `wl_keyboard.repeat_info`.
///
/// **The compositor generates no repeat events.** Since `wl_keyboard` v4 the
/// protocol makes repeat the *client's* job: the compositor advertises the rate
/// and delay it wants, and the client synthesises the repeats. So these two
/// constants are the whole of Parhelion's key-repeat implementation, and that is
/// correct, not a gap.
pub const KEY_REPEAT_DELAY_MS: i32 = 600;

/// Key repeat rate in keys per second (see [`KEY_REPEAT_DELAY_MS`]).
pub const KEY_REPEAT_RATE_HZ: i32 = 25;

// ==========================================================================
// Output constants (T7).
// ==========================================================================

/// The single output's name, as advertised to clients (`wl_output.name`).
/// One output is all M1 models; multi-output arrives with the DRM backend (M2),
/// which learns about real connectors.
pub const OUTPUT_NAME: &str = "parhelion-0";

/// Refresh rate advertised for the output's mode, in **millihertz** — 60 Hz.
///
/// It is a claim about pacing that M1 cannot yet keep: the render loop is
/// externally ticked and has no vblank (§4). Clients ask for a number and some
/// use it to schedule, so a plausible one is better than a zero; the real number
/// comes from the connector's mode with the DRM backend (M2).
pub const OUTPUT_REFRESH_MHZ: i32 = 60_000;

/// The output's size until a backend states its real one with
/// [`ProtocolHost::set_output_size`].
///
/// A compositor must advertise *something* the moment a client binds the output,
/// and the protocol host starts before any backend has told it how big its
/// window is. This is that placeholder — a plain 720p, chosen because it is
/// unremarkable. The nested backend replaces it at startup and on every resize.
pub const DEFAULT_OUTPUT_SIZE: (u32, u32) = (1280, 720);

// ==========================================================================
// Static Send/Sync regression guards (spike §5.5).
//
// These instantiate the exact trait bounds the Smithay-threading decision
// depends on. They are library (non-test) code assigned to `const` items, so
// `cargo build` type-checks them: a future Smithay / wayland-server bump that
// removed `Display: Send` (the fact that lets the dispatch thread own it) would
// break the BUILD here, not merely a test. The closures are never called; the
// compiler checks their bodies regardless (type-checking precedes dead-code
// elimination).
// ==========================================================================

/// Guards decision "2026-07-24 — Smithay threading fit": `Display<State>` is
/// `Send + Sync` *independent of `State`*, which is what lets the dispatch
/// thread own the `Display` while the scene is owned elsewhere.
const _GUARD_DISPLAY_SEND_SYNC: fn() = || {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<Display<State>>();
    assert_sync::<Display<State>>();
    assert_send::<DisplayHandle>();
    assert_sync::<DisplayHandle>();
};

/// Guards the same decision's requirement 2: the tokens that cross to the scene
/// are `Send`, and the [`SceneHandle`] the dispatch thread publishes through is
/// itself `Send` (it is moved into that thread). So the whole publish edge rides
/// a cross-thread message with no borrowed protocol state.
const _GUARD_TOKENS_SEND: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<SurfaceId>();
    assert_send::<ClientKey>();
    assert_send::<ProtocolEvent>();
    assert_send::<SceneHandle>();
};

/// Guards the T2 reverse edge: the [`FramePresenter`] crosses to T-render (a
/// different, frame-path thread), so it must be `Send + Sync`. It carries only a
/// `calloop` ping and an atomic — no Wayland object — which is exactly what keeps
/// "one thread touches protocol objects" true while still delivering callbacks.
const _GUARD_PRESENTER_SEND_SYNC: fn() = || {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<FramePresenter>();
    assert_sync::<FramePresenter>();
};

// ==========================================================================
// The reverse edge: render → dispatch "frame presented" notice.
// ==========================================================================

/// The render side's handle onto the dispatch thread's frame-callback machinery.
///
/// T-render ([`crate::render::RenderLoop`]) calls [`present`](Self::present) once
/// per produced frame; the dispatch thread then fires the `wl_surface.frame`
/// callbacks that were pending for that frame. Cloneable and `Send + Sync` so it
/// can live on the render thread (and be handed to future frame schedulers).
///
/// The notice is deliberately the smallest possible thing that crosses the
/// boundary: a `u32` timestamp and a ping. It carries no surface set in M1 — the
/// dispatch side fires *all* pending callbacks per tick (see [`present`]), which
/// is the honest v1 semantics until occlusion/visibility gating arrives with
/// damage in M2 (`docs/scene_graph_v1.md` §8).
#[derive(Clone)]
pub struct FramePresenter {
    /// Wakes the dispatch loop's [`PingSource`]. Coalescing: many pings before
    /// the loop drains collapse to one wakeup.
    ping: Ping,
    /// Latest presentation timestamp (monotonic ms, per the `wl_callback.done`
    /// protocol). Single slot: last writer wins, so the notice is bounded.
    ts: Arc<AtomicU32>,
}

impl FramePresenter {
    /// Notify the dispatch thread that a frame was presented at `time_ms`.
    ///
    /// Runs on T-render — a frame-path thread — so it must not block (I-1): it is
    /// a wait-free atomic store plus an eventfd write (`ping`), no lock shared
    /// with the dispatch thread, no synchronous reply. If several presents land
    /// before the dispatch thread drains, they coalesce (the ping edge collapses
    /// to one wakeup; the atomic holds the most recent timestamp).
    pub fn present(&self, time_ms: u32) {
        self.ts.store(time_ms, Ordering::Release);
        self.ping.ping();
    }
}

// ==========================================================================
// Control plane into the dispatch thread.
// ==========================================================================

/// Messages sent from [`ProtocolHost`] into its dispatch thread over a
/// `calloop` channel (which wakes the loop). This is how the accept seam and
/// shutdown reach the thread that owns the `Display`.
enum Control {
    /// Admit a client connected on this stream (the shard-assignment seam).
    AddClient(UnixStream),
    /// Start accepting clients on this bound Wayland socket (T6): the dispatch
    /// thread registers it as a `calloop` source and admits everything that
    /// connects. The socket is bound by the caller, so the display name is known
    /// without a round-trip.
    Listen(ListeningSocket),
    /// Apply one input event to the seat (T6) — the funnel's crossing into the
    /// dispatch thread. Producers (winit's loop, the test rig) never touch a
    /// protocol object themselves.
    Input(InputEvent),
    /// The backend's output changed size (T7): re-advertise the mode so clients
    /// learn the new screen size.
    OutputSize(u32, u32),
    /// Send `xdg_wm_base.ping` to every shell client with a live toplevel (T5).
    PingClients,
    /// Stop the dispatch loop and let the thread exit.
    Shutdown,
}

// ==========================================================================
// Per-client and per-shard protocol state (owned by the dispatch thread).
// ==========================================================================

/// Per-client data, stored in the client's `ClientData` so it is cleaned up
/// automatically on disconnect. Holds the Smithay compositor client state, the
/// core-assigned [`ClientKey`], and a [`SceneHandle`] so disconnect can publish
/// `ClientGone`.
struct ClientState {
    /// Required by `smithay::wayland::compositor` (per-client protocol state).
    compositor_state: CompositorClientState,
    /// This client's core-assigned key.
    key: ClientKey,
    /// Publish edge to the scene, so disconnect can emit `ClientGone`.
    scene: SceneHandle,
}

impl ClientData for ClientState {
    /// On disconnect, publish `ClientGone` so the scene drops this client's
    /// surfaces. Runs on the dispatch thread during `dispatch_clients`.
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        // Best-effort: `emit` ignores the error if the scene is shutting down.
        self.scene.emit(ProtocolEvent::ClientGone { client: self.key });
    }
}

/// The observability counters the dispatch thread publishes and [`ProtocolHost`]
/// reads.
///
/// They are grouped because they are one thing — "what the protocol side has
/// done so far" — shared by `Arc` between the two, and because passing four
/// separate atomics into the dispatch thread said nothing that this name does not
/// say better. None of them is load-bearing for dispatch: every one exists so a
/// test can assert on a number instead of a sleep.
#[derive(Clone)]
struct Counters {
    /// Frame callbacks committed but not yet fired, across all clients (T2's
    /// backpressure bound is per client; this is the total, for observability).
    pending_frame_callbacks: Arc<AtomicUsize>,
    /// Buffer bytes copied at commit — the damage-tracking evidence (T4).
    bytes_copied: Arc<AtomicUsize>,
    /// `xdg_wm_base.pong` replies received (T5).
    pongs_received: Arc<AtomicUsize>,
    /// `set_selection` requests accepted (T7b).
    selections_set: Arc<AtomicUsize>,
    /// Dispatch-loop iterations (M2 T0). The spin's measure: under the old
    /// aggregate-fd design a throttled client kept the loop turning with nothing
    /// to do, and this is the number that showed it.
    dispatch_iterations: Arc<AtomicUsize>,
}

impl Counters {
    /// A fresh set, all at zero.
    fn new() -> Self {
        Counters {
            pending_frame_callbacks: Arc::new(AtomicUsize::new(0)),
            bytes_copied: Arc::new(AtomicUsize::new(0)),
            pongs_received: Arc::new(AtomicUsize::new(0)),
            selections_set: Arc::new(AtomicUsize::new(0)),
            dispatch_iterations: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// A client's readiness source: the calloop registration for **our own clone** of
/// its socket, plus whether it is currently being polled.
///
/// The clone matters. wayland-backend keeps every client socket inside one epoll
/// fd of its own, and exposes no way to deregister a single client from it — which
/// is why the old design could only *skip* a throttled client, leaving its data
/// unread and the aggregate fd permanently ready (the spin). Owning a second
/// descriptor for the same socket gives us a registration we control.
struct ClientSource {
    /// The calloop registration, so it can be disabled, enabled, and removed.
    token: RegistrationToken,
    /// Whether the source is currently polled. Tracked rather than queried
    /// because calloop has no "is this enabled" accessor, and toggling an
    /// already-toggled source would be a wasted syscall per pass.
    enabled: bool,
}

/// What the dispatch thread tracks per live xdg toplevel. All of it is
/// protocol-side bookkeeping; the *canonical* facts (role, placement, size,
/// pixels) live in the scene (I-5) and this is only what is needed to drive the
/// protocol objects, which never leave this thread (§7).
struct ToplevelEntry {
    /// Smithay's handle for this toplevel — how configures are sent and the
    /// configured/acked state is queried.
    toplevel: ToplevelSurface,
    /// The C10 cascade placement assigned once, when the role was created, so a
    /// toplevel that unmaps and remaps returns to the same spot.
    placement: (i32, i32),
    /// Whether this toplevel currently has committed content (is *mapped*).
    /// Tracked here so the mapping commit — and only the mapping commit — sets
    /// the node's geometry: doing it on every commit would damage the whole
    /// extent each frame and undo T4's small-damage path.
    mapped: bool,
}

/// The dispatch shard's protocol state — thin, protocol-only. Owns no scene
/// data: its whole job is to translate protocol callbacks into [`ProtocolEvent`]s
/// published to the scene owner.
struct State {
    /// Smithay's compositor global/handler state.
    compositor_state: CompositorState,
    /// Smithay's `wl_shm` global/handler state (T3). Advertises the mandatory
    /// `argb8888`/`xrgb8888` formats and validates client pools/buffers; we reach
    /// the bytes through `smithay::wayland::shm` alone — no renderer type (the
    /// seam check, `docs/scene_graph_v1.md` §3).
    shm_state: ShmState,
    /// Smithay's `xdg_wm_base` global/handler state (T5). The frontend layer
    /// only — it validates the shell's request grammar and tracks configure
    /// serials; the window *meaning* (map, placement, damage) is ours below.
    xdg_shell_state: XdgShellState,
    /// Smithay's seat state (T6): owns the `wl_seat` global, capability
    /// advertisement, and keymap delivery.
    seat_state: SeatState<State>,
    /// The one seat (`CORE-BOUNDARY.md` C2), needed by name so the clipboard's
    /// focus can follow the keyboard's (T7).
    seat: Seat<State>,
    /// Smithay's data-device state (T7): the `wl_data_device_manager` global, the
    /// clipboard, and drag-and-drop.
    data_device_state: DataDeviceState,
    /// Smithay's output-manager state (T7). Held, not read: the request dispatch
    /// is delegated to `OutputManagerState`'s own impls (no accessor needed), so
    /// this value's job is to own the `zxdg_output_manager_v1` global's id for the
    /// dispatch thread's lifetime.
    #[allow(dead_code)]
    output_manager_state: OutputManagerState,
    /// The single output every surface lives on (T7). Real clients need one —
    /// they ask it for size, scale, and refresh before they draw — so it is
    /// implemented properly rather than stubbed: a real mode, real geometry, and
    /// `wl_surface.enter`/`leave` as surfaces map and unmap.
    output: Output,
    /// Keyboard handle — where [`InputEvent::Key`] events are applied, and the
    /// owner of the xkb state that turns keycodes into modifiers.
    keyboard: KeyboardHandle<State>,
    /// Pointer handle — motion/button/axis application and per-client focus.
    pointer: PointerHandle<State>,
    /// Latest pointer position in output coordinates, kept so a button or axis
    /// event knows where the cursor is without the source having to resend it.
    pointer_pos: (f64, f64),
    /// The focus routing table (§7's read-mostly replica; see
    /// [`crate::input::FocusMap`]). Updated by the same code that publishes a
    /// map/unmap to the scene, so it cannot drift independently.
    focus_map: FocusMap,
    /// The surface that currently has keyboard focus, so a re-focus that would
    /// change nothing sends no events.
    keyboard_focus: Option<SurfaceId>,
    /// The `Display`, owned here rather than by a calloop source (M2 T0).
    ///
    /// It used to live inside the aggregate `Generic` source, which handed it back
    /// as a callback argument. With per-client readiness sources there is no such
    /// argument, so the state owns it and each dispatch **takes it out and puts it
    /// back** — `dispatch_single_client` needs `&mut Display` and `&mut State` at
    /// once, and taking the `Option` is how those two borrows stop overlapping.
    display: Option<Display<State>>,
    /// Handle used to admit clients and resolve object→client.
    dh: DisplayHandle,
    /// Handle to this thread's `calloop` loop, so a source callback can register
    /// *another* source — specifically, the listening socket that arrives after
    /// the loop is already running (T6).
    loop_handle: LoopHandle<'static, State>,
    /// Monotonic source of [`SurfaceId`]s.
    next_surface_id: u64,
    /// Monotonic source of [`ClientKey`]s.
    next_client_key: u64,
    /// Maps live protocol surfaces to their core id, for commit/destroy lookup.
    obj_to_surface: HashMap<ObjectId, SurfaceId>,
    /// The reverse direction (T6): from a core id back to its protocol object,
    /// so input routed by [`FocusMap`] (which speaks core tokens only) can reach
    /// the `wl_surface` it belongs to.
    sid_to_obj: HashMap<SurfaceId, ObjectId>,
    /// Live `wl_surface` handles, keyed by object id — the set [`present`] drains
    /// frame callbacks from, and the set the per-client backlog is computed over.
    /// The `WlSurface` never leaves this thread (it is not `Send`), which is the
    /// whole point: protocol objects stay on the dispatch thread (§7).
    surfaces: HashMap<ObjectId, WlSurface>,
    /// The pixel block currently shown for each surface, retained here so a
    /// content commit can **partial-copy** (patch only the damaged region) instead
    /// of re-copying the whole buffer (T4). Shared by `Arc` with the scene, so
    /// patching goes through `Arc::make_mut` (copy-on-write: an in-flight
    /// snapshot's pixels are never mutated).
    surface_pixels: HashMap<ObjectId, Arc<PixelBuffer>>,
    /// Live xdg toplevels, keyed by their `wl_surface`'s object id (T5) — the
    /// lookup `commit` uses to tell "this surface is a toplevel" from "this
    /// surface is a bare `wl_surface`", which is now the difference between
    /// content that can be displayed and content that cannot.
    toplevels: HashMap<ObjectId, ToplevelEntry>,
    /// Monotonic counter feeding the C10 cascade (`CASCADE_*`). Never reset, so
    /// placements are a deterministic function of creation order within a run.
    next_toplevel_index: u64,
    /// The publish edge to the scene owner.
    scene: SceneHandle,
    /// Latest presentation timestamp from the render side (shared with the
    /// [`FramePresenter`]); read by [`present`] when firing callbacks.
    present_ts: Arc<AtomicU32>,
    /// Observability counters, shared with the host (see [`Counters`]).
    counters: Counters,
    /// One readiness source per client (M2 T0) — the shape that makes throttling
    /// literal: a throttled client's source is *disabled*, so its socket is not
    /// polled at all until it drains.
    client_sources: HashMap<ClientId, ClientSource>,
    /// Set when a surface was destroyed, so the selection is re-checked once the
    /// departing client's teardown has finished (see [`refresh_selection`]).
    ///
    /// [`refresh_selection`]: State::refresh_selection
    selection_needs_refresh: bool,
    /// Set by a `Shutdown` control message to end the loop.
    stop: bool,
}

impl State {
    /// Allocate the next surface id.
    fn alloc_surface_id(&mut self) -> SurfaceId {
        let id = SurfaceId(self.next_surface_id);
        self.next_surface_id += 1;
        id
    }

    /// Allocate the next client key.
    fn alloc_client_key(&mut self) -> ClientKey {
        let key = ClientKey(self.next_client_key);
        self.next_client_key += 1;
        key
    }

    /// The next C10 fallback placement (see the `CASCADE_*` constants). Pure
    /// function of the creation index: deterministic, clock-free, and unaffected
    /// by what else is on screen. **Temporary** — the policy daemon (S1) takes
    /// this over in M4.
    fn alloc_cascade_placement(&mut self) -> (i32, i32) {
        let step = (self.next_toplevel_index % CASCADE_WRAP) as i32;
        self.next_toplevel_index += 1;
        (
            CASCADE_ORIGIN_X + step * CASCADE_STEP_X,
            CASCADE_ORIGIN_Y + step * CASCADE_STEP_Y,
        )
    }

    /// This surface's current title and app_id, as the client last set them.
    /// Reads the role attributes Smithay maintains; the values are copied out so
    /// only owned data crosses to the scene.
    fn toplevel_metadata(surface: &WlSurface) -> (Option<String>, Option<String>) {
        with_states(surface, |states| {
            let attrs = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .expect("toplevel surface has xdg role data")
                .lock()
                .expect("xdg role data lock");
            (attrs.title.clone(), attrs.app_id.clone())
        })
    }

    /// Walk the surfaces whose state just became effective — this surface and its
    /// subsurface tree — collecting their changes (M2 T7).
    ///
    /// The traversal is Smithay's tree, read faithfully rather than re-derived:
    /// child order comes from [`ordered_children`], which returns the parent's
    /// children *with the parent's own slot in place*, because `place_below` makes
    /// "beneath the parent" a real position.
    fn collect_tree(&mut self, surface: &WlSurface, updates: &mut Vec<SurfaceUpdate>, depth: usize) {
        if depth > MAX_SUBSURFACE_DEPTH {
            return;
        }
        self.collect_surface(surface, updates);

        let ordered = ordered_children(surface);
        // A surface with no subsurfaces is the whole tree; nothing to order.
        if ordered.len() <= 1 {
            return;
        }
        if let Some(&parent_id) = self.obj_to_surface.get(&surface.id()) {
            let order: Vec<SurfaceId> = ordered
                .iter()
                .filter_map(|s| self.obj_to_surface.get(&s.id()).copied())
                .collect();
            updates.push(SurfaceUpdate::Order {
                parent: parent_id,
                order,
            });
        }
        for child in ordered {
            if child.id() != surface.id() {
                self.collect_tree(&child, updates, depth + 1);
            }
        }
    }

    /// Collect one surface's effective state: its subsurface position, and the
    /// content it just committed (if any).
    fn collect_surface(&mut self, surface: &WlSurface, updates: &mut Vec<SurfaceUpdate>) {
        let obj = surface.id();
        let Some(&sid) = self.obj_to_surface.get(&obj) else {
            return;
        };

        // A subsurface's position is double-buffered and applies with the parent's
        // commit in both sync and desync mode — reading the *current* state here
        // is exactly that rule, because "current" is what the commit just made.
        if get_role(surface) == Some(SUBSURFACE_ROLE) {
            let location = with_states(surface, |states| {
                states
                    .cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location
            });
            updates.push(SurfaceUpdate::Position {
                surface: sid,
                offset: (location.x, location.y),
            });
        }

        let (assignment, raw_damage) = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let current = guard.current();
            (current.buffer.take(), std::mem::take(&mut current.damage))
        });

        match assignment {
            Some(BufferAssignment::NewBuffer(buffer)) => {
                // Buffer==surface coordinates in M1 (no scale/transform yet); this
                // is the marked site where the two are merged (M2+ generalizes it).
                let damage_rects = damage_to_rects(&raw_damage);
                let prev = self.surface_pixels.get(&obj).cloned();
                match build_pixel_block(&buffer, prev, &damage_rects) {
                    Ok(Some((block, opaque, content_damage, bytes))) => {
                        self.counters.bytes_copied.fetch_add(bytes, Ordering::Relaxed);
                        self.surface_pixels.insert(obj.clone(), block.clone());
                        let size = (block.width, block.height);

                        // The mapping commit of a *toplevel* also carries its C10
                        // placement; later commits must not touch geometry, or
                        // every frame would damage the whole extent. Subsurfaces
                        // are placed by their parent-relative position instead.
                        if let Some(entry) = self.toplevels.get_mut(&obj)
                            && !entry.mapped
                        {
                            entry.mapped = true;
                            updates.push(SurfaceUpdate::Geometry {
                                surface: sid,
                                offset: entry.placement,
                                size,
                            });
                        }
                        updates.push(SurfaceUpdate::Content {
                            surface: sid,
                            size,
                            source: TextureSource::Shm(block),
                            opaque,
                            damage: content_damage,
                        });
                    }
                    // Non-shm buffer or zero-size: nothing to show (no dmabuf in M1).
                    Ok(None) => {}
                    // Access error: Smithay already posted the protocol error /
                    // killed the client; nothing more to do.
                    Err(_) => {}
                }
                buffer.release();
            }
            // Null attach: unmap — the node loses its source; drop the retained block.
            Some(BufferAssignment::Removed) => {
                self.surface_pixels.remove(&obj);
                updates.push(SurfaceUpdate::Unmap { surface: sid });
            }
            // No buffer change this commit: the node keeps its current pixels.
            None => {}
        }
    }

    /// The per-surface consequences of a commit that the *tree* does not share:
    /// input routing, focus, and the toplevel's unmap bookkeeping. Returns whether
    /// this commit unmapped the surface.
    fn post_commit_bookkeeping(&mut self, obj: &ObjectId) -> bool {
        let Some(&sid) = self.obj_to_surface.get(obj) else {
            return false;
        };
        let mapped_now = self.surface_pixels.contains_key(obj);

        if let Some(entry) = self.toplevels.get_mut(obj) {
            if !mapped_now && entry.mapped {
                entry.mapped = false;
                self.leave_output(obj);
                self.focus_map.unmap(sid);
                self.refocus_keyboard();
                return true;
            }
            if mapped_now {
                self.enter_output(obj);
            }
        }
        // Routing first, then focus: `refocus_keyboard` reads the table this
        // rebuilds, so the order is the dependency.
        self.refresh_input_routing();
        self.refocus_keyboard();
        false
    }

    /// Unmap a toplevel's node: it loses its source (and so its visibility) with
    /// the structural damage `clear_source` raises, and it leaves the focus
    /// routing table — what cannot be seen cannot be clicked or focused. Used by
    /// both unmap paths: null attach and `xdg_toplevel.destroy`.
    fn unmap_surface(&mut self, obj: &ObjectId) {
        self.surface_pixels.remove(obj);
        // Off screen means off the output (T7) — the mirror of the `enter` sent
        // when it mapped. Also idempotent.
        if let Some(surface) = self.surfaces.get(obj).cloned() {
            self.output.leave(&surface);
        }
        if let Some(&sid) = self.obj_to_surface.get(obj) {
            self.scene.mutate(move |s| s.clear_source(sid));
            self.focus_map.unmap(sid);
            self.refocus_keyboard();
        }
    }

    /// Tell the output a surface is on it (idempotent).
    fn enter_output(&self, obj: &ObjectId) {
        if let Some(surface) = self.surfaces.get(obj) {
            self.output.enter(surface);
        }
    }

    /// Tell the output a surface has left it (idempotent).
    fn leave_output(&self, obj: &ObjectId) {
        if let Some(surface) = self.surfaces.get(obj) {
            self.output.leave(surface);
        }
    }

    /// Rebuild the input routing table from the surface trees (M2 T7).
    ///
    /// Subsurfaces are hit-testable, so the table can no longer be a flat list of
    /// toplevel rects. It is rebuilt from the same walk the scene flattens with —
    /// each mapped toplevel's tree in composition order — and `z` is simply the
    /// index in that order, so "topmost" means the same thing to input as it does
    /// to the compositor.
    ///
    /// It is rebuilt wholesale rather than patched because the inputs are cheap
    /// (a handful of surfaces), and because a patch would be a second
    /// implementation of the ordering rules with its own chance to disagree —
    /// the exact bug class the shared walk exists to prevent.
    ///
    /// Runs on the dispatch thread from its own state: no scene round-trip, so
    /// input routing never waits on the scene (the I-2 discipline from T6).
    fn refresh_input_routing(&mut self) {
        self.focus_map.clear();
        let mut z = 0i32;

        // Toplevels are the roots; their cascade placement is their origin.
        //
        // **Sorted by `SurfaceId`, and that is load-bearing.** `toplevels` is a
        // HashMap, so iterating it gives an arbitrary order — and since `z` here is
        // the index in composition order, an unsorted walk would make "topmost"
        // (and therefore keyboard focus) depend on hash iteration. All toplevels
        // share z = 0 in the scene, whose tiebreak is ascending `SurfaceId`: the
        // most recently mapped window is on top. This reproduces exactly that, so
        // input and pixels agree.
        let mut roots: Vec<(SurfaceId, ObjectId, (i32, i32))> = self
            .toplevels
            .iter()
            .filter(|(_, e)| e.mapped)
            .filter_map(|(obj, e)| {
                self.obj_to_surface
                    .get(obj)
                    .map(|sid| (*sid, obj.clone(), e.placement))
            })
            .collect();
        roots.sort_by_key(|(sid, _, _)| *sid);
        let roots: Vec<(ObjectId, (i32, i32))> = roots
            .into_iter()
            .map(|(_, obj, placement)| (obj, placement))
            .collect();

        for (root_obj, origin) in roots {
            let Some(root) = self.surfaces.get(&root_obj).cloned() else {
                continue;
            };
            for (obj, offset) in self.flatten_for_input(&root, origin) {
                let Some(&sid) = self.obj_to_surface.get(&obj) else {
                    continue;
                };
                let Some(block) = self.surface_pixels.get(&obj) else {
                    // No pixels, no input: the T5 rule, applied through the tree.
                    // foot's border subsurface lives here — role, position, no
                    // buffer, and so click-transparent.
                    continue;
                };
                let rect = Rect::new(
                    offset.0,
                    offset.1,
                    block.width as i32,
                    block.height as i32,
                );
                let focusable = self.toplevels.contains_key(&obj);
                self.focus_map.map(sid, rect, z, focusable);
                z += 1;
            }
        }
    }

    /// Flatten one surface tree into `(object, absolute offset)` pairs in
    /// composition order — the input-side twin of the scene's flattening.
    fn flatten_for_input(
        &self,
        surface: &WlSurface,
        origin: (i32, i32),
    ) -> Vec<(ObjectId, (i32, i32))> {
        let mut out = Vec::new();
        self.flatten_for_input_inner(surface, origin, &mut out, 0);
        out
    }

    /// Depth-bounded recursion behind [`flatten_for_input`].
    fn flatten_for_input_inner(
        &self,
        surface: &WlSurface,
        offset: (i32, i32),
        out: &mut Vec<(ObjectId, (i32, i32))>,
        depth: usize,
    ) {
        if depth > MAX_SUBSURFACE_DEPTH {
            return;
        }
        let ordered = ordered_children(surface);
        if ordered.len() <= 1 {
            out.push((surface.id(), offset));
            return;
        }
        for child in ordered {
            if child.id() == surface.id() {
                out.push((surface.id(), offset));
                continue;
            }
            let location = with_states(&child, |states| {
                states
                    .cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location
            });
            let child_offset = (offset.0 + location.x, offset.1 + location.y);
            self.flatten_for_input_inner(&child, child_offset, out, depth + 1);
        }
    }

    /// The live `wl_surface` for a core id, if it still exists. The bridge from
    /// the focus table's core tokens back to protocol objects — which never leave
    /// this thread (§7).
    fn wl_surface_for(&self, surface: SurfaceId) -> Option<WlSurface> {
        let obj = self.sid_to_obj.get(&surface)?;
        self.surfaces.get(obj).filter(|s| s.is_alive()).cloned()
    }

    /// Apply the **C10 focus fallback**: keyboard focus is the topmost mapped
    /// toplevel. Called whenever that set changes (map, unmap, destroy, client
    /// gone). A re-focus that would change nothing sends nothing, so a client
    /// does not see spurious `leave`/`enter` pairs.
    ///
    /// **Loudly temporary.** "Focus follows topmost" is a policy decision — a
    /// reasonable user might want click-to-focus, focus-follows-mouse, or a
    /// tiling scheme — and `CORE-BOUNDARY.md` §4 rule 4 puts policy in a server.
    /// The reference policy daemon S1 takes this over in **M4**; it lives here
    /// now only because a compositor nothing can be typed into is not a
    /// compositor (C10: the core stays usable with every server dead).
    /// Re-state the clipboard focus, which makes Smithay re-broadcast the
    /// selection — and, crucially, **notice that its source has died**.
    ///
    /// Smithay clears a dead selection lazily: it checks whether the source is
    /// still alive only when the selection is next sent, which normally happens
    /// on a focus change. So when the clipboard's owner dies while focus does
    /// *not* change (a background client exits; the focused window is untouched),
    /// nobody notices, and the focused client is left holding an offer backed by
    /// a corpse — a paste that answers with nothing.
    ///
    /// **Timing matters.** This must run *after* the departing client's teardown,
    /// not during it: the `destroyed` hook for a surface fires while that
    /// client's other objects — including its data source — are still alive, so a
    /// liveness check there would conclude the selection is healthy and
    /// re-broadcast a dying offer. Hence the `selection_needs_refresh` flag,
    /// drained at the end of the dispatch pass.
    fn refresh_selection(&mut self) {
        let dh = self.dh.clone();
        let seat = self.seat.clone();
        let focused = self
            .keyboard_focus
            .and_then(|sid| self.wl_surface_for(sid))
            .and_then(|s| s.client());
        // Smithay re-broadcasts only when the clipboard focus *changes*, so
        // restating the same client would do nothing. Clearing it first forces
        // the re-broadcast; no client observes the intermediate state, because
        // with no focus there is nobody the selection is sent to.
        set_data_device_focus(&dh, &seat, None);
        set_data_device_focus(&dh, &seat, focused);
    }

    fn refocus_keyboard(&mut self) {
        let target = self.focus_map.topmost();
        if target == self.keyboard_focus {
            return;
        }
        self.keyboard_focus = target;
        let surface = target.and_then(|sid| self.wl_surface_for(sid));

        // The clipboard follows the keyboard (T7). Only the focused client may
        // set the selection — that is the protocol's own answer to "who is
        // allowed to overwrite what the user copied", and it is why this call
        // belongs *here*, in the one place focus changes, rather than anywhere a
        // clipboard is mentioned.
        let dh = self.dh.clone();
        let seat = self.seat.clone();
        let focused_client = surface.as_ref().and_then(|s| s.client());
        set_data_device_focus(&dh, &seat, focused_client);

        let keyboard = self.keyboard.clone();
        let serial = SERIAL_COUNTER.next_serial();
        // Sends leave to the old focus and enter to the new one, in that order.
        keyboard.set_focus(self, surface, serial);
    }
}

/// Admit a client on `stream`: give it a core [`ClientKey`], the per-client
/// protocol state, and a scene handle for its `ClientGone`, then insert it on
/// this thread's `Display` — the shard-assignment seam (requirement 1).
///
/// One function, two callers: the [`ProtocolHost::add_client`] control message
/// (the rig's socketpairs) and the listening socket's accept loop (real clients,
/// T6). Both must produce identical state, so they share the code rather than
/// resembling each other.
fn admit_client(state: &mut State, stream: UnixStream) {
    // Our own descriptor for the same socket, used purely as a readiness signal.
    // wayland-backend keeps the one it is given inside its own epoll and offers no
    // way to deregister a single client from it; this clone is the registration we
    // control, and therefore the thing throttling can switch off.
    let readiness = match stream.try_clone() {
        Ok(fd) => fd,
        Err(_) => return, // out of descriptors: admit nothing rather than half-admit
    };

    let key = state.alloc_client_key();
    let data = ClientState {
        compositor_state: CompositorClientState::default(),
        key,
        scene: state.scene.clone(),
    };
    let Ok(client) = state.dh.insert_client(stream, Arc::new(data)) else {
        return;
    };
    let id = client.id();

    // One source per client: readiness here means "this client has requests", and
    // the callback dispatches that client alone.
    let dispatch_id = id.clone();
    let token = state.loop_handle.insert_source(
        Generic::new(readiness, Interest::READ, Mode::Level),
        move |_readiness, _fd, state: &mut State| {
            Ok(dispatch_one_client(state, dispatch_id.clone()))
        },
    );
    // On registration failure the loop is tearing down; the client is admitted but
    // never polled, which is the same outcome as not admitting it.
    if let Ok(token) = token {
        state.client_sources.insert(
            id,
            ClientSource {
                token,
                enabled: true,
            },
        );
    }
}

// ==========================================================================
// The input funnel's dispatch-side half (T6).
// ==========================================================================

/// Apply one [`InputEvent`] to the seat. Runs on the dispatch thread — the only
/// thread that may touch protocol objects (§7) — reached by a control message
/// from whatever produced the event (winit's loop, or a test).
///
/// Every event gets a fresh serial from the shared counter, so serials are
/// monotonic across keyboard and pointer alike (clients use them to order
/// requests against events, and to prove a request answers a specific event).
///
/// **I-2 holds by construction here:** nothing in this function consults the
/// render side or the scene thread. Pointer routing reads the dispatch thread's
/// own [`FocusMap`] replica, so delivery cannot queue behind a frame.
fn apply_input(state: &mut State, event: InputEvent) {
    match event {
        InputEvent::Key {
            code,
            pressed,
            time_ms,
        } => {
            let keyboard = state.keyboard.clone();
            let serial = SERIAL_COUNTER.next_serial();
            let key_state = if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            };
            // xkb keycodes are evdev + 8 (the X11 inheritance). Smithay sends
            // `raw - 8` back on the wire, so the client sees the evdev code it
            // expects — the funnel speaks evdev at both ends.
            keyboard.input::<(), _>(
                state,
                Keycode::new(code + 8),
                key_state,
                serial,
                time_ms,
                // No compositor keybindings in M1: every key is forwarded to the
                // focused client. Grabs and shortcuts are policy (S1, M4).
                |_, _, _| FilterResult::Forward,
            );
        }
        InputEvent::PointerMotion { x, y, time_ms } => {
            state.pointer_pos = (x, y);
            // Hit-test the replica, then translate the core id to the object.
            // Smithay is given the focused surface's **origin** in global space
            // (it derives the client's surface-local coordinates from it), which
            // is why [`Hit`] names origin and local separately.
            let focus = state.focus_map.at(x, y).and_then(|hit| {
                state
                    .wl_surface_for(hit.surface)
                    .map(|s| (s, Point::<f64, Logical>::from(hit.origin)))
            });
            let pointer = state.pointer.clone();
            let serial = SERIAL_COUNTER.next_serial();
            // `motion` emits the leave/enter pair itself when the focus changes,
            // always before the motion event — which is what makes
            // "enter before input" true without us sequencing it by hand.
            pointer.motion(
                state,
                focus,
                &MotionEvent {
                    location: Point::from((x, y)),
                    serial,
                    time: time_ms,
                },
            );
            pointer.frame(state);
        }
        InputEvent::PointerButton {
            button,
            pressed,
            time_ms,
        } => {
            let pointer = state.pointer.clone();
            let serial = SERIAL_COUNTER.next_serial();
            // Goes to whatever the last motion focused — no re-hit-test, because
            // a button must be delivered to the surface that received the enter,
            // even if geometry changed underneath in between.
            pointer.button(
                state,
                &ButtonEvent {
                    serial,
                    time: time_ms,
                    button,
                    state: if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    },
                },
            );
            pointer.frame(state);
        }
        InputEvent::PointerAxis {
            horizontal,
            vertical,
            steps,
            time_ms,
        } => {
            let pointer = state.pointer.clone();
            // M1's only axis source is a stepped wheel (winit's line/pixel deltas
            // are normalised by the backend); touchpad kinetic scrolling needs
            // libinput and arrives with it (M2).
            let mut frame = AxisFrame::new(time_ms).source(AxisSource::Wheel);
            if horizontal != 0.0 {
                frame = frame.value(Axis::Horizontal, horizontal);
                if steps != 0 {
                    // v120 is the protocol's high-resolution step unit: one
                    // physical wheel click is 120.
                    frame = frame.v120(Axis::Horizontal, steps * 120);
                }
            }
            if vertical != 0.0 {
                frame = frame.value(Axis::Vertical, vertical);
                if steps != 0 {
                    frame = frame.v120(Axis::Vertical, steps * 120);
                }
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    /// `xdg_surface.get_toplevel`: the surface has taken the toplevel role.
    ///
    /// The role goes to the scene immediately — it is canonical state (I-5) — but
    /// the node stays *invisible* until a buffer commits: role + source is what
    /// "mapped" means (`crate::scene::NodeRole`). The C10 placement is assigned
    /// here, once, so an unmap/remap cycle returns to the same spot.
    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let obj = toplevel.wl_surface().id();
        let placement = self.alloc_cascade_placement();
        if let Some(&sid) = self.obj_to_surface.get(&obj) {
            self.scene
                .mutate(move |s| s.set_role(sid, NodeRole::Toplevel(ToplevelRole::default())));
        }
        self.toplevels.insert(
            obj,
            ToplevelEntry {
                toplevel,
                placement,
                mapped: false,
            },
        );
    }

    /// `xdg_toplevel.destroy`: unmap, and drop the role — the `wl_surface` may
    /// outlive its role object, and a roleless surface is never displayed. The
    /// node itself stays live until the `wl_surface` is destroyed.
    fn toplevel_destroyed(&mut self, toplevel: ToplevelSurface) {
        let obj = toplevel.wl_surface().id();
        self.toplevels.remove(&obj);
        self.unmap_surface(&obj);
        if let Some(&sid) = self.obj_to_surface.get(&obj) {
            self.scene.mutate(move |s| s.set_role(sid, NodeRole::None));
        }
    }

    /// `xdg_toplevel.set_title` → canonical state. Metadata only (T5): nothing
    /// in the core branches on it.
    fn title_changed(&mut self, toplevel: ToplevelSurface) {
        let Some(&sid) = self.obj_to_surface.get(&toplevel.wl_surface().id()) else {
            return;
        };
        let (title, _) = State::toplevel_metadata(toplevel.wl_surface());
        self.scene.mutate(move |s| s.set_title(sid, title));
    }

    /// `xdg_toplevel.set_app_id` → canonical state. Metadata only, as
    /// [`title_changed`](Self::title_changed).
    fn app_id_changed(&mut self, toplevel: ToplevelSurface) {
        let Some(&sid) = self.obj_to_surface.get(&toplevel.wl_surface().id()) else {
            return;
        };
        let (_, app_id) = State::toplevel_metadata(toplevel.wl_surface());
        self.scene.mutate(move |s| s.set_app_id(sid, app_id));
    }

    /// `xdg_wm_base.pong`: the client answered a ping and is alive. Counted for
    /// observability; M1 takes no action on an unresponsive client (that is
    /// policy, and it needs a timer the core does not run yet).
    fn client_pong(&mut self, _client: ShellClient) {
        self.counters.pongs_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Popups are **out of scope for M1** (`docs/plans/m1_tasks.md` T5). This
    /// trait method has no default, so it must exist; dismissing the popup at
    /// once (`popup_done`) is the honest answer — the client learns immediately
    /// that it has no popup, instead of waiting on a configure that a
    /// half-implementation would never send correctly.
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        surface.send_popup_done();
    }

    /// A popup grab, likewise dismissed — see [`new_popup`](Self::new_popup).
    fn grab(&mut self, surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        surface.send_popup_done();
    }

    /// Repositioning applies to popups only, which we dismiss on creation, so
    /// there is nothing here to reposition.
    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Every admitted client carries a ClientState (set in `add_client`).
        &client
            .get_data::<ClientState>()
            .expect("admitted client has ClientState")
            .compositor_state
    }

    /// A surface was created: assign it a [`SurfaceId`], remember the mapping,
    /// and publish `SurfaceCreated` attributed to the owning client's key.
    fn new_surface(&mut self, surface: &WlSurface) {
        let sid = self.alloc_surface_id();
        self.obj_to_surface.insert(surface.id(), sid);
        self.sid_to_obj.insert(sid, surface.id());
        // Keep the object handle so the reverse edge ([`present`]) can drain this
        // surface's frame callbacks. Cheap clone (an Arc-backed resource id).
        self.surfaces.insert(surface.id(), surface.clone());

        // The owning client's core key lives in its ClientState.
        let client = surface.client().expect("surface has a live client");
        let key = client
            .get_data::<ClientState>()
            .expect("admitted client has ClientState")
            .key;

        self.scene.emit(ProtocolEvent::SurfaceCreated {
            surface: sid,
            client: key,
        });
    }

    /// A surface became a subsurface of `parent` (M2 T7).
    ///
    /// The role and the parent link go to the scene immediately — they are
    /// canonical state (I-5) — but the child stays unmapped until it commits
    /// content *and* its parent chain is mapped. New subsurfaces are placed above
    /// their parent, which is what the protocol specifies.
    fn new_subsurface(&mut self, surface: &WlSurface, parent: &WlSurface) {
        let (Some(&child_id), Some(&parent_id)) = (
            self.obj_to_surface.get(&surface.id()),
            self.obj_to_surface.get(&parent.id()),
        ) else {
            return;
        };
        self.scene
            .mutate(move |s| s.attach_subsurface(child_id, parent_id));
    }

    /// A surface committed: publish `SurfaceCommitted`, and apply the
    /// double-buffered buffer state (attach) if this commit carried one.
    ///
    /// Copy-at-commit (T3), damage-aware (T4): the buffer's pixels are copied here,
    /// on the dispatch thread where the Wayland objects live (§7), into an owned
    /// scene-side [`PixelBuffer`]; the `wl_buffer` is **released immediately** after
    /// so single-buffer clients can reuse it. With a prior block and client damage,
    /// only the damaged region is copied (partial copy, copy-on-write via
    /// [`build_pixel_block`]). The copy is a memcpy on the dispatch thread — *not*
    /// the frame path (I-1).
    ///
    /// **xdg-shell lifecycle (T5).** For a surface carrying the toplevel role
    /// this is where the protocol's rules are enforced, in order: a buffer on a
    /// surface that has not acked its configure is a protocol error and maps
    /// nothing; the first buffer commit maps the toplevel at its C10 placement;
    /// a null attach unmaps it and rearms the initial-configure dance; and any
    /// commit that leaves the toplevel unconfigured is answered with a
    /// `configure`.
    fn commit(&mut self, surface: &WlSurface) {
        let Some(&sid) = self.obj_to_surface.get(&surface.id()) else {
            return;
        };
        self.scene.emit(ProtocolEvent::SurfaceCommitted { surface: sid });

        // **A synchronized subsurface's commit is not effective yet** (M2 T7).
        // Its state is cached and becomes current when its nearest desynchronized
        // ancestor commits — that is the whole point of sync mode, and it is why
        // a client can update a window and its decorations without the compositor
        // ever showing half of it. Smithay owns the caching; we own not acting
        // early.
        if is_sync_subsurface(surface) {
            return;
        }

        // The xdg gate: a toplevel may not commit a buffer before acking its
        // initial configure. Peeked rather than taken, because the buffer is
        // consumed below by the tree walk.
        let has_buffer = with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer
                .is_some()
        });
        if has_buffer
            && let Some(entry) = self.toplevels.get(&surface.id())
            && !entry.toplevel.ensure_configured()
        {
            return;
        }

        // Collect this surface and every subsurface whose state just became
        // effective, then apply the lot in **one** scene message. The atomicity is
        // the semantic heart of sync mode; splitting it into a message per surface
        // would let a snapshot land in the middle.
        let mut updates = Vec::new();
        self.collect_tree(surface, &mut updates, 0);
        if !updates.is_empty() {
            self.scene.mutate(move |s| s.apply_commit(updates));
        }

        // Unmap bookkeeping and the xdg dance are per-surface concerns of the
        // committed surface itself, not of its tree.
        let obj = surface.id();
        let unmapped = self.post_commit_bookkeeping(&obj);

        // The initial configure, sent in response to the client's buffer-less
        // "here I am" commit. Size 0×0 and no states means "you choose" — the core
        // has no size policy to impose (§4 rule 4); the placement it does own is
        // C10's cascade.
        if let Some(entry) = self.toplevels.get(&obj)
            && !entry.toplevel.is_initial_configure_sent()
        {
            entry.toplevel.send_configure();
        }

        // Re-arm the dance *after* that check, so the unmapping commit itself
        // earns no configure: per xdg-shell an unmapped surface must perform the
        // initial commit/configure sequence again, and the configure belongs to
        // that future commit, not to this one.
        if unmapped
            && let Some(entry) = self.toplevels.get(&obj)
        {
            entry.toplevel.reset_initial_configure_sent();
        }
    }

    /// A surface was destroyed: drop the mappings and publish `SurfaceDestroyed`.
    ///
    /// This also covers client disconnect — wayland-server destroys every object
    /// a departing client owned, so it is the one place surfaces leave the focus
    /// routing table no matter how they go.
    fn destroyed(&mut self, surface: &WlSurface) {
        self.surfaces.remove(&surface.id());
        self.surface_pixels.remove(&surface.id());
        self.toplevels.remove(&surface.id());
        if let Some(sid) = self.obj_to_surface.remove(&surface.id()) {
            self.sid_to_obj.remove(&sid);
            self.focus_map.unmap(sid);
            self.scene.emit(ProtocolEvent::SurfaceDestroyed { surface: sid });
            self.refocus_keyboard();
            // The departing surface's client may have owned the clipboard. The
            // check cannot happen *here*: this hook runs partway through that
            // client's teardown, and its `wl_data_source` is still alive, so a
            // liveness test would say the selection is fine and re-broadcast a
            // dying offer. Defer it to the end of the dispatch pass, by which
            // time the whole client is gone.
            self.selection_needs_refresh = true;
        }
    }
}

// `delegate_compositor!` supplies the Dispatch/GlobalDispatch impls for
// wl_compositor/wl_surface/wl_subcompositor/wl_region/wl_callback, routing them
// to `CompositorState` and the `CompositorHandler` impl above. The subsurface
// tripwire lives in [`State::commit`], not in a hand-written replacement for this
// macro — see the note there.
smithay::delegate_compositor!(State);

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for State {
    /// A client destroyed a `wl_buffer`. We copy-at-commit and release
    /// immediately, so we never hold a buffer past a commit — nothing to do here.
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

/// The seat's handler side (T6). Focus targets are `WlSurface` — the thing a
/// Wayland client actually receives input on, and the type Smithay implements
/// the keyboard/pointer/touch target traits for directly.
///
/// Both callbacks are deliberately empty:
///
/// - `focus_changed` — the core *sets* focus (C10 policy, [`refocus_keyboard`]),
///   so being told about it afterwards adds nothing in M1. When focus is
///   S1's decision (M4) this is where the control plane would learn of it.
/// - `cursor_image` — a client asked to set the cursor. **Accepted and ignored
///   for rendering**: in the nested backend the host desktop draws the cursor,
///   and the hardware cursor plane is M2 (`CORE-BOUNDARY.md` C1). Ignoring it is
///   not a protocol violation — the request is honoured by being accepted — but
///   the client's chosen shape is not shown yet.
impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<State>, _focused: Option<&WlSurface>) {}

    fn cursor_image(&mut self, _seat: &Seat<State>, _image: CursorImageStatus) {}
}

/// Nothing to do when a client binds the output: Smithay sends the geometry,
/// mode, scale, and `done` events itself from the state we set at creation. The
/// hook exists for compositors that track per-client output objects; we do not
/// need to, because the output never changes identity — only its mode does, and
/// `change_current_state` re-broadcasts that to everyone bound.
impl OutputHandler for State {}

// `delegate_output!` supplies the Dispatch/GlobalDispatch impls for wl_output
// (and zxdg_output_manager_v1), routing them to `OutputManagerState`.
smithay::delegate_output!(State);

/// The clipboard and drag-and-drop (T7), through Smithay's data-device machinery.
///
/// # Why this is in the core at all
///
/// The clipboard is not a shell feature — it is a service the display server owes
/// every client, and clients treat its absence as a broken compositor (`foot`
/// refuses to start without `wl_data_device_manager`). It is protocol machinery
/// (C3) and it needs canonical per-seat state, so it belongs here rather than in a
/// server.
///
/// # What is deliberately *not* here yet
///
/// **Access is ungated.** Any client that has keyboard focus may read and write
/// the selection — which is ordinary Wayland, and which invariant **I-7** will
/// eventually make a capability question ("no privileged operation proceeds
/// without a grant attached to the client's security context"). The capability
/// machinery itself is C8 and arrives with the microkernel milestone (M4); a
/// remote (Rayland) client must land on the restricted side of it. Until then the
/// honest description is: implemented, correct, and not yet policed. Recorded in
/// the decision log so it is a scheduled debt rather than an oversight.
///
/// The handler methods below are all defaults: `SelectionHandler`'s hooks matter
/// only for *compositor-provided* selections (we set none — clipboard content
/// passes client to client, and the compositor never reads it), and the two DnD
/// grab handlers only for compositor-initiated drags. Smithay drives the
/// client-to-client transfer itself.
impl SelectionHandler for State {
    /// No compositor-provided selections in M1, so there is no per-selection data
    /// to carry.
    type SelectionUserData = ();

    /// A client set (or cleared) the selection. The core does not look at the
    /// content — the bytes go client-to-client through a pipe and never touch the
    /// compositor — so this only counts the event, for observability
    /// (`ProtocolHost::selections_set`). A test waiting for "the clipboard is
    /// ready" needs a definite condition, and a transient copy tool's window is
    /// not one: it can map and vanish between two polls.
    fn new_selection(
        &mut self,
        _ty: SelectionTarget,
        _source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        self.counters.selections_set.fetch_add(1, Ordering::Relaxed);
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

/// **Drag-and-drop is deferred, and says so out loud.**
///
/// A client that starts a drag has its source **cancelled immediately**. That is
/// protocol-legal — a compositor may cancel a drag at any time, and `cancelled`
/// is exactly the event a client is built to handle — and it is the honest shape
/// of "not yet": the client learns at once that no drag is happening, instead of
/// waiting on a drag that will never produce an enter, a drop, or a cancel.
///
/// Why not implement it: a real drag is a **pointer grab**, and how grabs compose
/// with Parhelion's focus model (C10 today, S1's policy in M4) is a design
/// conversation, not an afternoon's plumbing. Smithay would supply the grab
/// machinery, but the compositor-side semantics — what a drag does to focus, what
/// happens when the drag source's client dies mid-drag, how a shaped or
/// 3D-transformed window hit-tests during one — are ours to decide. Half of that,
/// shipped quietly, is the same lie in a different costume.
///
/// The clipboard, by contrast, is fully implemented: it needs no grab, and every
/// client expects it to work.
impl ClientDndGrabHandler for State {
    fn started(&mut self, source: Option<WlDataSource>, _icon: Option<WlSurface>, _seat: Seat<Self>) {
        if let Some(source) = source {
            source.cancelled();
        }
    }
}

/// Server-initiated drags (the compositor starting a drag itself) are never
/// begun, so none of these can fire. Left at their defaults deliberately: an
/// implementation for an event that cannot occur is dead code pretending to be
/// coverage.
impl ServerDndGrabHandler for State {}

// `delegate_data_device!` supplies the Dispatch/GlobalDispatch impls for
// wl_data_device_manager/wl_data_device/wl_data_source/wl_data_offer.
smithay::delegate_data_device!(State);

// `delegate_seat!` supplies the Dispatch/GlobalDispatch impls for
// wl_seat/wl_keyboard/wl_pointer/wl_touch, routing them to `SeatState` (which
// owns capability advertisement, keymap delivery, and per-client focus
// bookkeeping) and the `SeatHandler` impl above.
smithay::delegate_seat!(State);

// `delegate_xdg_shell!` supplies the Dispatch/GlobalDispatch impls for
// xdg_wm_base/xdg_surface/xdg_toplevel/xdg_popup/xdg_positioner, routing them to
// `XdgShellState` (which validates the shell grammar, tracks configure serials,
// and posts the role/serial protocol errors) and the `XdgShellHandler` impl
// above. Frontend only: no renderer type crosses this seam either.
smithay::delegate_xdg_shell!(State);

// `delegate_shm!` supplies the Dispatch/GlobalDispatch impls for
// wl_shm/wl_shm_pool/wl_buffer, routing them to `ShmState` (which validates
// pools/buffers) and the `ShmHandler`/`BufferHandler` impls above. This is the
// entire seam: `smithay::wayland::shm`; Smithay's renderer layer is untouched.
smithay::delegate_shm!(State);

/// A surface's immediate children **in composition order, with the surface's own
/// slot included** (M2 T7).
///
/// Smithay's `get_children` filters the parent's self-marker out of the list,
/// which loses exactly the information `place_below` creates: whether a child is
/// beneath or above its parent. This walks one level of the tree instead —
/// `SkipChildren` stops the traversal descending, while still visiting each child
/// — and the parent appears at its own position, which is the ordering the scene
/// stores.
fn ordered_children(parent: &WlSurface) -> Vec<WlSurface> {
    let mut out = Vec::new();
    with_surface_tree_upward(
        parent,
        (),
        |surface, _, _| {
            if surface == parent {
                TraversalAction::DoChildren(())
            } else {
                // Visit this child, but do not descend into its own subtree: one
                // level is what "immediate children" means.
                TraversalAction::SkipChildren
            }
        },
        |surface, _, _| out.push(surface.clone()),
        |_, _, _| true,
    );
    out
}

/// Convert Smithay's accumulated surface damage into our surface-coordinate
/// rects. **The marked buffer==surface site (constraint 2):** with no buffer
/// scale or transform in M1, `Damage::Surface` and `Damage::Buffer` rectangles
/// are the same coordinate space, so both map to a plain [`Rect`]; when scale /
/// transform arrive (M2+), the `Buffer` arm is where the conversion goes.
fn damage_to_rects(damage: &[Damage]) -> Vec<Rect> {
    damage
        .iter()
        .map(|d| match d {
            Damage::Surface(r) => Rect::new(r.loc.x, r.loc.y, r.size.w, r.size.h),
            Damage::Buffer(r) => Rect::new(r.loc.x, r.loc.y, r.size.w, r.size.h),
        })
        .collect()
}

/// What [`build_pixel_block`] returns on success: the new (possibly CoW-cloned)
/// pixel block, whether it is opaque, the damage to hand the scene, and the bytes
/// copied from the buffer.
type BuiltBlock = (Arc<PixelBuffer>, bool, ContentDamage, usize);

/// Build the surface's new pixel block from an attached `wl_shm` buffer, copying
/// only what is needed. Runs on the dispatch thread inside `commit`.
///
/// **Partial copy + copy-on-write (T4).** When there is a `prev` block of the
/// same dimensions and the client posted real, sub-surface damage, only the
/// damaged rects are copied — `Arc::make_mut` clones the block first *iff* it is
/// still shared (an in-flight snapshot holds it), so shared pixels are never
/// mutated. Otherwise (no prior block, dimensions changed, no/covers-all damage)
/// it full-copies. Returns the block, whether it is opaque, the resulting
/// [`ContentDamage`] for the scene, and the bytes copied from the buffer.
///
/// Format handling lives *only* here (the seam): `argb8888` → straight RGBA, not
/// opaque (blend); `xrgb8888` → RGBA, alpha forced 255, opaque (overwrite).
/// `wl_shm` bytes are little-endian, so each pixel is `[B, G, R, A/X]` in memory;
/// we reorder to `[R, G, B, A]` and strip the stride.
///
/// Returns `Ok(None)` for a non-shm buffer, an unsupported format, a zero-size
/// buffer, or geometry that does not fit the pool (a hostile-input guard — a
/// trust boundary, so reject rather than index out of bounds).
///
/// `unsafe` is confined to the one documented pool read; allowed with the SAFETY
/// justification at the block (same pattern as `pump_display`).
#[allow(unsafe_code)]
fn build_pixel_block(
    buffer: &WlBuffer,
    prev: Option<Arc<PixelBuffer>>,
    damage_rects: &[Rect],
) -> Result<Option<BuiltBlock>, BufferAccessError> {
    with_buffer_contents(buffer, |ptr, len, data| {
        let (width, height) = (data.width.max(0) as usize, data.height.max(0) as usize);
        let stride = data.stride.max(0) as usize;
        let offset = data.offset.max(0) as usize;
        if width == 0 || height == 0 {
            return None;
        }
        // Per-format decode rule; unadvertised formats cannot occur (Smithay
        // rejects them at buffer creation), but reject defensively.
        let (opaque, has_alpha) = match data.format {
            Format::Xrgb8888 => (true, false),
            Format::Argb8888 => (false, true),
            _ => return None,
        };
        // Hostile-input guard: the buffer must fit within the mapped pool.
        if stride < width * 4 || offset.saturating_add(height * stride) > len {
            return None;
        }
        // SAFETY: `ptr`/`len` describe the pool mapping, valid for the duration of
        // this callback (the `with_buffer_contents` contract). We only read within
        // `[0, len)` — enforced by the guard above — and copy out immediately; the
        // slice does not outlive the callback. A client concurrently mutating its
        // own shm can only produce torn pixels, never a memory-safety fault.
        let pool = unsafe { std::slice::from_raw_parts(ptr, len) };
        let read_px = |x: usize, y: usize| -> [u8; 4] {
            let p = offset + y * stride + x * 4;
            let (b, g, r) = (pool[p], pool[p + 1], pool[p + 2]);
            let a = if has_alpha { pool[p + 3] } else { 255 };
            [r, g, b, a]
        };

        // Clip client damage to the buffer; drop empties.
        let buf_rect = Rect::new(0, 0, width as i32, height as i32);
        let clipped: Vec<Rect> = damage_rects
            .iter()
            .map(|r| r.intersect(&buf_rect))
            .filter(|r| !r.is_empty())
            .collect();
        let dims_match =
            prev.as_ref().is_some_and(|p| p.width as usize == width && p.height as usize == height);
        let covers_all = clipped
            .iter()
            .any(|r| r.x == 0 && r.y == 0 && r.w as usize == width && r.h as usize == height);

        if dims_match && !clipped.is_empty() && !covers_all {
            // Partial copy-on-write: clone the prior block only if shared, then
            // patch just the damaged rects.
            let mut arc = prev.expect("dims_match implies Some");
            let block = Arc::make_mut(&mut arc);
            let mut bytes = 0usize;
            for r in &clipped {
                for y in r.y as usize..r.bottom() as usize {
                    let drow = y * width * 4;
                    for x in r.x as usize..r.right() as usize {
                        let d = drow + x * 4;
                        block.rgba[d..d + 4].copy_from_slice(&read_px(x, y));
                    }
                }
                bytes += r.area() * 4;
            }
            Some((arc, opaque, ContentDamage::Rects(clipped), bytes))
        } else {
            // Full copy (map, resize, or no usable client damage).
            let mut rgba = Vec::with_capacity(width * height * 4);
            for y in 0..height {
                for x in 0..width {
                    rgba.extend_from_slice(&read_px(x, y));
                }
            }
            let block = Arc::new(PixelBuffer {
                width: width as u32,
                height: height as u32,
                rgba,
            });
            Some((block, opaque, ContentDamage::Full, width * height * 4))
        }
    })
}

// ==========================================================================
// The ProtocolHost handle.
// ==========================================================================

/// The protocol frontend, seen from the rest of the core (and the harness).
///
/// Owns the control channel and join handle for the dispatch thread; the
/// dispatch thread publishes surface lifecycle to the [`SceneHandle`] it was
/// given at construction. Dropping this shuts the thread down cleanly.
pub struct ProtocolHost {
    /// Control channel into the dispatch thread (admit client / shutdown).
    control_tx: CalloopSender<Control>,
    /// The dispatch thread, joined on drop.
    dispatch: Option<JoinHandle<()>>,
    /// The render side's notice handle (cloned out via [`frame_presenter`]).
    ///
    /// [`frame_presenter`]: ProtocolHost::frame_presenter
    presenter: FramePresenter,
    /// Observability counters, shared with the dispatch thread (see the
    /// accessors below).
    counters: Counters,
}

impl ProtocolHost {
    /// Start a `shards = 1` protocol host publishing to `scene`: spin up the
    /// dispatch thread (which creates the `Display`, advertises `wl_compositor`,
    /// and runs the calloop loop) and return a handle to it. Surface lifecycle
    /// from any admitted client is emitted into `scene`; frame callbacks are
    /// fired when the render side calls [`FramePresenter::present`].
    pub fn new(scene: SceneHandle) -> Self {
        let (control_tx, control_rx) = calloop_channel::<Control>();
        // The reverse-edge plumbing: a ping wakes the dispatch loop, an atomic
        // carries the latest presentation timestamp. Both halves are created
        // here so the FramePresenter (render side) and the dispatch thread share
        // them.
        let (ping, ping_source) = make_ping().expect("create frame-present ping");
        let present_ts = Arc::new(AtomicU32::new(0));
        let counters = Counters::new();

        let presenter = FramePresenter {
            ping,
            ts: present_ts.clone(),
        };
        let counters_for_thread = counters.clone();

        let dispatch = std::thread::Builder::new()
            .name("parhelion-proto-0".into())
            .spawn(move || {
                run_dispatch(
                    control_rx,
                    scene,
                    ping_source,
                    present_ts,
                    counters_for_thread,
                )
            })
            .expect("spawn dispatch thread");

        ProtocolHost {
            control_tx,
            dispatch: Some(dispatch),
            presenter,
            counters,
        }
    }

    /// The accept seam (requirement 1). Admit a client connected on `stream` —
    /// an external `ListeningSocket::accept` loop and the test rig both call
    /// this; it routes the stream to a shard (the only shard, at `shards = 1`).
    pub fn add_client(&self, stream: UnixStream) {
        // If the dispatch thread is gone the host is being torn down; drop it.
        let _ = self.control_tx.send(Control::AddClient(stream));
    }

    /// Feed one input event into the seat (T6) — the funnel's public door.
    ///
    /// Callable from any thread: winit's event loop on the main thread, a test
    /// on the test thread. It only *enqueues* onto the control channel and wakes
    /// the dispatch thread, which owns every protocol object (§7) and does the
    /// actual delivery. Non-blocking, so an input source is never made to wait on
    /// the compositor (**I-2**).
    pub fn input(&self, event: InputEvent) {
        let _ = self.control_tx.send(Control::Input(event));
    }

    /// Bind a Wayland socket in `$XDG_RUNTIME_DIR` (`wayland-1`, `wayland-2`, …)
    /// and start accepting clients on it. Returns the display name to put in
    /// `WAYLAND_DISPLAY`.
    ///
    /// The socket is bound *here*, on the caller's thread, so the name is known
    /// immediately; only the accepting is handed to the dispatch thread. That
    /// keeps this a plain fallible call instead of a cross-thread round-trip.
    pub fn listen_auto(&self) -> std::io::Result<String> {
        let socket = ListeningSocket::bind_auto("wayland", 1..33)
            .map_err(|e| std::io::Error::other(format!("bind wayland socket: {e}")))?;
        let name = socket
            .socket_name()
            .expect("bind_auto always names its socket")
            .to_string_lossy()
            .into_owned();
        let _ = self.control_tx.send(Control::Listen(socket));
        Ok(name)
    }

    /// Bind a Wayland socket at an explicit path and start accepting clients on
    /// it. Used where `$XDG_RUNTIME_DIR` should not be involved — notably tests,
    /// which bind inside their own temporary directory and so neither depend on
    /// the environment nor collide with a real session.
    pub fn listen_at(&self, path: impl Into<std::path::PathBuf>) -> std::io::Result<()> {
        let socket = ListeningSocket::bind_absolute(path.into())
            .map_err(|e| std::io::Error::other(format!("bind wayland socket: {e}")))?;
        let _ = self.control_tx.send(Control::Listen(socket));
        Ok(())
    }

    /// Tell the compositor how big its output is (T7) — the backend's window
    /// size in the nested case, a connector's mode in M2. Re-advertises the mode
    /// to every client bound to `wl_output`.
    ///
    /// Asynchronous like every other control message; nothing waits for it.
    pub fn set_output_size(&self, width: u32, height: u32) {
        let _ = self.control_tx.send(Control::OutputSize(width, height));
    }

    /// A clone of the render side's notice handle. Hand this to the
    /// [`RenderLoop`](crate::render::RenderLoop) so its ticks fire frame
    /// callbacks (the reverse edge).
    pub fn frame_presenter(&self) -> FramePresenter {
        self.presenter.clone()
    }

    /// Total frame callbacks currently pending across all clients, as of the
    /// dispatch thread's last pass. Observability for the backpressure test; the
    /// bound this stays under (per client) is [`MAX_PENDING_FRAME_CALLBACKS`].
    pub fn pending_frame_callbacks(&self) -> usize {
        self.counters.pending_frame_callbacks.load(Ordering::Relaxed)
    }

    /// Total buffer bytes copied at commit so far — the damage-tracking counter.
    /// A small-damage commit on a large surface copies far fewer than the whole
    /// buffer (partial copy); the proportionality test asserts on the delta.
    pub fn bytes_copied(&self) -> usize {
        self.counters.bytes_copied.load(Ordering::Relaxed)
    }

    /// Send `xdg_wm_base.ping` to every shell client that has a live toplevel —
    /// the liveness half of ping/pong (T5). Asynchronous like every other control
    /// message: this enqueues, the dispatch thread sends, and the client's `pong`
    /// shows up in [`pongs_received`](Self::pongs_received). Nothing in the core
    /// waits for it (I-3's spirit: no synchronous round-trip anywhere).
    ///
    /// M1 has no ping *scheduler* — the unresponsive-client policy (when to ping,
    /// what to do about silence) is policy, not core, and needs S1 (M4). This
    /// exists so the protocol side is complete and testable.
    pub fn ping_clients(&self) {
        let _ = self.control_tx.send(Control::PingClients);
    }

    /// Number of `xdg_wm_base.pong` replies received so far.
    pub fn pongs_received(&self) -> usize {
        self.counters.pongs_received.load(Ordering::Relaxed)
    }

    /// How many times the dispatch loop has turned (M2 T0).
    ///
    /// The spin's measure. Under the old aggregate-fd design, a throttled client's
    /// unread data kept the loop's source permanently ready, so this climbed
    /// without bound during a flood while no useful work happened. With per-client
    /// sources the throttled client is not polled at all, and the loop sleeps.
    pub fn dispatch_iterations(&self) -> usize {
        self.counters.dispatch_iterations.load(Ordering::Relaxed)
    }

    /// Number of `set_selection` requests the compositor has accepted — i.e. how
    /// many times the clipboard changed hands. Observability for tests waiting on
    /// "the clipboard is ready"; the content itself never passes through the
    /// core, and this counter says nothing about it.
    pub fn selections_set(&self) -> usize {
        self.counters.selections_set.load(Ordering::Relaxed)
    }
}

impl Drop for ProtocolHost {
    fn drop(&mut self) {
        // Ask the loop to stop, then join so the thread (and its Display) is
        // torn down before we return — crash-only teardown ordering matters.
        let _ = self.control_tx.send(Control::Shutdown);
        if let Some(handle) = self.dispatch.take() {
            let _ = handle.join();
        }
    }
}

// ==========================================================================
// The dispatch thread body.
// ==========================================================================

/// Sum a surface's pending (committed, not-yet-fired) frame callbacks. Reads the
/// *current* half of the double-buffered surface state — a `wl_surface.frame`
/// request lands in *pending* and is merged into *current* on commit, so this
/// counts exactly the callbacks that are eligible to fire on the next tick.
fn surface_frame_backlog(surface: &WlSurface) -> usize {
    with_states(surface, |data| {
        data.cached_state
            .get::<SurfaceAttributes>()
            .current()
            .frame_callbacks
            .len()
    })
}

/// Dispatch one client's pending requests (M2 T0).
///
/// Called from that client's own readiness source, so "which client has data" is
/// answered by the event loop rather than by us asking every client in turn.
///
/// The `Display` is taken out of the state for the call and put back after:
/// `dispatch_single_client` wants `&mut Display` *and* `&mut State`, and those two
/// borrows cannot overlap while the display lives in the state. **Does not
/// flush** — flushing is owned by the single site in [`run_dispatch`].
fn dispatch_one_client(state: &mut State, id: ClientId) -> PostAction {
    let Some(mut display) = state.display.take() else {
        // Only possible if this were re-entered from inside a dispatch, which
        // calloop does not do. Bail rather than panic: a compositor that aborts on
        // an impossible condition is worse than one that skips a wakeup.
        return PostAction::Continue;
    };
    let result = display.backend().dispatch_single_client(state, id.clone());
    state.display = Some(display);

    // Housekeeping that used to run once per aggregate pass, now per dispatch:
    // drop object handles for surfaces whose client is gone, so backlog and
    // callback bookkeeping never touch a dead resource, and drop their retained
    // pixel blocks (a client that disconnects without destroying its surfaces).
    state.surfaces.retain(|_, s| s.is_alive());
    state
        .surface_pixels
        .retain(|obj, _| state.surfaces.contains_key(obj));

    // A dispatch error means the client is gone (EOF, or it was killed for a
    // protocol error). Its socket would otherwise stay readable-at-EOF forever,
    // and a level-triggered source over a dead socket is a spin of its own — so
    // the source is removed here, at the one place that learns of the death.
    if result.is_err() {
        remove_client_source(state, &id);
    }

    // A surface was destroyed during this dispatch; now that the client's teardown
    // is complete, re-check whether it took the clipboard with it (T7b).
    if std::mem::take(&mut state.selection_needs_refresh) {
        state.refresh_selection();
    }

    update_throttles(state);
    republish_backlog(state);
    PostAction::Continue
}

/// Total pending frame callbacks per client — the throttle signal.
fn per_client_backlog(state: &State) -> HashMap<ClientId, usize> {
    let mut backlog: HashMap<ClientId, usize> = HashMap::new();
    for surface in state.surfaces.values() {
        if let Some(client) = surface.client() {
            *backlog.entry(client.id()).or_default() += surface_frame_backlog(surface);
        }
    }
    backlog
}

/// Publish the total pending-callback count for observability (the rig's
/// backpressure test reads it through [`ProtocolHost::pending_frame_callbacks`]).
fn republish_backlog(state: &State) {
    let total: usize = state.surfaces.values().map(surface_frame_backlog).sum();
    state
        .counters
        .pending_frame_callbacks
        .store(total, Ordering::Relaxed);
}

/// Apply the backpressure policy (I-10's fairness rider), now literally.
///
/// A client at or over [`MAX_PENDING_FRAME_CALLBACKS`] has its readiness source
/// **disabled**: the event loop stops polling that socket entirely, so its
/// requests stay in the kernel buffer and its own writes block once that fills.
/// It is re-enabled once its backlog drains below
/// [`RESUME_PENDING_FRAME_CALLBACKS`] — the hysteresis gap that stops a steady
/// flooder from toggling the registration on every tick.
///
/// **This is what ends the spin.** The old design could only *skip* a throttled
/// client while the aggregate fd stayed ready, so the loop turned continuously
/// with nothing to do. With the client's own source disabled there is no readiness
/// to report, and the loop sleeps: the fix is structural, not a bound on how fast
/// we spin.
fn update_throttles(state: &mut State) {
    let backlog = per_client_backlog(state);
    let handle = state.loop_handle.clone();
    for (id, source) in state.client_sources.iter_mut() {
        let pending = backlog.get(id).copied().unwrap_or(0);
        let over = pending >= MAX_PENDING_FRAME_CALLBACKS;
        let drained = pending < RESUME_PENDING_FRAME_CALLBACKS;
        if source.enabled && over && handle.disable(&source.token).is_ok() {
            source.enabled = false;
        } else if !source.enabled && drained && handle.enable(&source.token).is_ok() {
            source.enabled = true;
        }
    }
}

/// Drop a departed client's readiness source and forget it.
fn remove_client_source(state: &mut State, id: &ClientId) {
    if let Some(source) = state.client_sources.remove(id) {
        state.loop_handle.remove(source.token);
    }
}

/// The reverse edge's dispatch-side half: a frame was presented, so fire the
/// `wl_surface.frame` callbacks that were pending for it. Runs on the dispatch
/// thread (the ping source's callback), which is the only thread allowed to
/// touch these protocol objects (§7).
///
/// **v1 semantics (documented as v1, `docs/scene_graph_v1.md` §8):** every tick
/// fires *all* pending callbacks on *every* surface — visible or not. That is
/// required, not a shortcut: a client may commit a frame request on an
/// unmapped/attach-less surface and must still get its `done`. Occlusion- and
/// visibility-aware throttling (only firing for surfaces actually presented)
/// needs damage/visibility and arrives in M2 (T4). The timestamp is whatever the
/// render side last stored — deterministic in tests, monotonic ms in production.
fn present(state: &mut State) {
    let time = state.present_ts.load(Ordering::Acquire);
    for surface in state.surfaces.values() {
        if !surface.is_alive() {
            continue;
        }
        // Drain and fire: `wl_callback` is one-shot, so taking the vec both
        // sends `done` and destroys the callback (it is inert afterwards). This
        // is why a second tick with no new frame request delivers nothing more.
        let callbacks: Vec<WlCallback> = with_states(surface, |data| {
            std::mem::take(
                &mut data
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .frame_callbacks,
            )
        });
        for callback in callbacks {
            callback.done(time);
        }
    }
    // Everything pending just fired.
    state.counters.pending_frame_callbacks.store(0, Ordering::Relaxed);
}

/// Run the shard-0 dispatch loop: own the `Display`, advertise `wl_compositor`,
/// and pump calloop until a `Shutdown` control message arrives. Surface
/// lifecycle is published to `scene`; frame callbacks are fired when a present
/// notice arrives on `ping_source` (timestamp read from `present_ts`).
fn run_dispatch(
    control_rx: Channel<Control>,
    scene: SceneHandle,
    ping_source: PingSource,
    present_ts: Arc<AtomicU32>,
    counters: Counters,
) {
    let display: Display<State> = Display::new().expect("create wayland display");
    let dh = display.handle();

    // The loop is created before the state because the state holds a handle to
    // it: sources that arrive *after* the loop is running (the listening socket,
    // T6) are registered from inside a source callback, which needs the handle.
    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("create calloop event loop");
    let handle = event_loop.handle();
    let compositor_state = CompositorState::new::<State>(&dh);
    // **`wl_subcompositor`: advertised, used by real clients, and NOT composited.**
    // A stated debt, blocking on M2 T7 — and the measurement below is the reason
    // T0's tripwire is not here.
    //
    // The scene composites only root surfaces; a subsurface's content is dropped.
    // T0 set out to make that refusal loud (decision log: "advertise before
    // support requires loud refusal at point of use"). Both refusal points were
    // implemented and measured against `foot`, and both kill it:
    //
    //   * refusing `get_subsurface` — foot creates **nine** subsurfaces during
    //     startup, so it dies at once;
    //   * refusing a subsurface that commits a buffer — **eight** of those nine
    //     carry real pixels (its client-side decorations), so it dies a moment
    //     later.
    //
    // (An earlier session reported "foot calls `get_subsurface` zero times". That
    // was wrong — a `WAYLAND_DEBUG` grep matching `@` where the format uses `#` —
    // and the correction is recorded in the decision log. The whole tripwire
    // design rested on it.)
    //
    // So there is no refusal point that keeps an honest client alive: foot both
    // requires the global and uses it for content. Loud refusal and a working
    // terminal are mutually exclusive until subsurfaces are real (M2 T7), and the
    // milestone's own acceptance criterion is the terminal.
    //
    // **Consequence, stated:** foot renders today **without its decorations** —
    // title bar, borders, corners are all subsurfaces we silently drop. That is
    // the silent wrongness the decision exists to forbid, and it stands, visibly,
    // until T7 pays it.

    // Advertise `wl_shm`. The mandatory `argb8888`/`xrgb8888` formats are added by
    // `ShmState::new`; we request no extras (T3 handles exactly those two).
    let shm_state = ShmState::new::<State>(&dh, std::iter::empty());
    // Advertise `xdg_wm_base` (T5) with Smithay's default wm capabilities. We
    // implement none of the states those capabilities describe yet (maximize,
    // fullscreen, minimize, window menu are out of scope for M1) — advertising
    // beyond the defaults is deliberately not done here.
    let xdg_shell_state = XdgShellState::new::<State>(&dh);

    // The seat (T6): one seat, keyboard + pointer, no touch. Creating it here
    // advertises the `wl_seat` global with those capabilities; Smithay delivers
    // the keymap to each client that binds a keyboard.
    let mut seat_state = SeatState::<State>::new();
    let mut seat = seat_state.new_wl_seat(&dh, SEAT_NAME);
    let keyboard = seat
        .add_keyboard(
            // An explicit `us` layout rather than the empty default: the keymap
            // must be the same on every machine or the rig's keycode assertions
            // would depend on the developer's xkb configuration.
            XkbConfig {
                layout: "us",
                ..XkbConfig::default()
            },
            KEY_REPEAT_DELAY_MS,
            KEY_REPEAT_RATE_HZ,
        )
        .expect("compile the default xkb keymap");
    let pointer = seat.add_pointer();

    // The data device (T7): clipboard and drag-and-drop. Real applications treat
    // a compositor without one as broken — `foot` refuses to start — because the
    // clipboard is not a feature of the desktop shell, it is a service the
    // display server owes every client.
    let data_device_state = DataDeviceState::new::<State>(&dh);

    // The output (T7). A real client asks the compositor how big its screen is,
    // how it is scaled, and how fast it refreshes, *before* it draws anything —
    // so this is a real output with a real mode, not an advertised shell. Its
    // size tracks the backend's (the nested window's, via `Control::OutputSize`).
    let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            // Zero physical size is the honest answer for a nested window: there
            // is no monitor, so there are no millimetres. Clients treat 0 as
            // "unknown" rather than computing a nonsense DPI from a made-up one.
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Parhelion".into(),
            model: "Nested".into(),
        },
    );
    output.create_global::<State>(&dh);
    let mode = OutputMode {
        size: (DEFAULT_OUTPUT_SIZE.0 as i32, DEFAULT_OUTPUT_SIZE.1 as i32).into(),
        refresh: OUTPUT_REFRESH_MHZ,
    };
    output.set_preferred(mode);
    // Scale 1: fractional scaling needs `wp_fractional_scale` and a renderer that
    // can honour it (M2+). Claiming a scale we do not implement would make every
    // client draw at the wrong size.
    output.change_current_state(
        Some(mode),
        Some(smithay::utils::Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );

    let mut state = State {
        display: Some(display),
        compositor_state,
        shm_state,
        xdg_shell_state,
        seat_state,
        seat: seat.clone(),
        data_device_state,
        output_manager_state,
        output,
        keyboard,
        pointer,
        pointer_pos: (0.0, 0.0),
        focus_map: FocusMap::new(),
        keyboard_focus: None,
        dh: dh.clone(),
        loop_handle: handle.clone(),
        client_sources: HashMap::new(),
        next_surface_id: 0,
        next_client_key: 0,
        obj_to_surface: HashMap::new(),
        sid_to_obj: HashMap::new(),
        surfaces: HashMap::new(),
        surface_pixels: HashMap::new(),
        toplevels: HashMap::new(),
        next_toplevel_index: 0,
        scene: scene.clone(),
        present_ts,
        counters,
        selection_needs_refresh: false,
        stop: false,
    };

    // NOTE (M2 T0): the `Display`'s aggregate poll fd is **not** registered. Client
    // readiness comes from the per-client sources created in `admit_client`, which
    // is what makes throttling a real deregistration rather than a skipped read.
    // Grep-verifiable: the display's fd is registered with no calloop source.

    // Control channel: admit clients (the accept seam) and shutdown. The stop
    // flag lives in `State` so both this closure (via its `&mut State` argument)
    // and the dispatch loop below observe the same value.
    handle
        .insert_source(control_rx, |event, _, state: &mut State| {
            if let ChannelEvent::Msg(control) = event {
                match control {
                    Control::AddClient(stream) => admit_client(state, stream),
                    // One ping serial for the whole sweep; a client with several
                    // toplevels is pinged once (the extra `send_ping`s report
                    // "already pending" and are ignored — the ping is per shell
                    // client, not per window). Enqueue only; the loop's single
                    // flush site pushes it.
                    // A bound socket arrived: register it so everything that
                    // connects is admitted through the same seam as the rig's
                    // socketpair clients (`add_client`).
                    Control::Listen(socket) => {
                        let listen_handle = state.loop_handle.clone();
                        let inserted = listen_handle.insert_source(
                            Generic::new(socket, Interest::READ, Mode::Level),
                            |_readiness, socket, state: &mut State| {
                                // Accept everything queued; `accept` yields None
                                // when the backlog is drained.
                                while let Ok(Some(stream)) = socket.accept() {
                                    admit_client(state, stream);
                                }
                                Ok(PostAction::Continue)
                            },
                        );
                        // A failed registration means the loop is tearing down;
                        // dropping the socket unbinds it, which is the right
                        // outcome (no half-listening compositor).
                        let _ = inserted;
                    }
                    // The input funnel's crossing (T6): apply on this thread,
                    // where the seat's protocol objects live.
                    Control::Input(event) => apply_input(state, event),
                    // The output resized (T7). `change_current_state` re-sends
                    // mode + done to every client bound to the output, so a
                    // client that laid itself out for the old size learns the
                    // new one. The scene's own damage on resize is the backend's
                    // business (it owns the frame); this is only the protocol
                    // half.
                    Control::OutputSize(w, h) => {
                        let mode = OutputMode {
                            size: (w as i32, h as i32).into(),
                            refresh: OUTPUT_REFRESH_MHZ,
                        };
                        state.output.set_preferred(mode);
                        state.output.change_current_state(Some(mode), None, None, None);
                    }
                    Control::PingClients => {
                        let serial = SERIAL_COUNTER.next_serial();
                        for toplevel in state.xdg_shell_state.toplevel_surfaces() {
                            let _ = toplevel.client().send_ping(serial);
                        }
                    }
                    Control::Shutdown => state.stop = true,
                }
            }
        })
        .expect("register control source");

    // The reverse edge: a present notice from the render side. Firing the
    // callbacks only *enqueues* the `done` events; they leave on the flush below.
    handle
        .insert_source(ping_source, |_event, _metadata, state: &mut State| {
            present(state);
            // Firing callbacks is what drains a throttled client's backlog — and a
            // throttled client's source is disabled, so it can never re-arm from
            // its own dispatch. This is the *only* place the re-arm can happen,
            // which is why it lives here rather than only after a dispatch.
            update_throttles(state);
            republish_backlog(state);
        })
        .expect("register frame-present ping source");

    // A modest timeout keeps the loop responsive to the stop flag and re-checks
    // level-triggered readiness — it is a safety net, not the primary wakeup
    // (all sources wake the loop).
    while !state.stop {
        // Counted for the no-spin assertion (M2 T0): under the old aggregate-fd
        // design a throttled client kept this loop turning with nothing to do, and
        // this number is how the flooding test proves it no longer does.
        state
            .counters
            .dispatch_iterations
            .fetch_add(1, Ordering::Relaxed);

        event_loop
            .dispatch(Some(Duration::from_millis(20)), &mut state)
            .expect("calloop dispatch");

        // THE ONE FLUSH SITE (flush ownership, `docs/scene_graph_v1.md` §8).
        // Every source callback this iteration only *enqueues* bytes — client
        // replies in `pump_display`, `wl_callback.done` in `present` — and this
        // is the sole place they are pushed to the sockets. Keeping it here, once
        // per iteration after all sources ran, is what makes "one flush" true and
        // grep-verifiable: there is no other `flush_clients` in the core.
        let _ = state.dh.flush_clients();
    }
}
