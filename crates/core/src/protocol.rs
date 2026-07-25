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
    generic::{Generic, NoIoDrop},
    ping::{make_ping, Ping, PingSource},
    EventLoop, Interest, LoopHandle, Mode, PostAction,
};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_callback::WlCallback;
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
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    with_states, BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
    Damage, SurfaceAttributes,
};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ShellClient, ToplevelSurface, XdgShellHandler, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{with_buffer_contents, BufferAccessError, ShmHandler, ShmState};

use crate::input::{FocusMap, InputEvent};
use crate::scene::{
    ClientKey, ContentDamage, NodeRole, PixelBuffer, ProtocolEvent, Rect, SceneHandle, SurfaceId,
    TextureSource, ToplevelRole, Transform,
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
    /// Total pending frame callbacks across all clients, republished each pass
    /// for observability (`ProtocolHost::pending_frame_callbacks`, used by the
    /// backpressure rig test). Not load-bearing for dispatch.
    pending_frame_callbacks: Arc<AtomicUsize>,
    /// Running total of buffer bytes copied at commit — the damage-tracking
    /// counter that shows partial copies copy less than the whole buffer
    /// (`ProtocolHost::bytes_copied`). Not load-bearing for dispatch.
    bytes_copied: Arc<AtomicUsize>,
    /// Count of `xdg_wm_base.pong` replies received (T5 liveness check). Pure
    /// observability — nothing acts on an unresponsive client in M1.
    pongs_received: Arc<AtomicUsize>,
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

    /// Unmap a toplevel's node: it loses its source (and so its visibility) with
    /// the structural damage `clear_source` raises, and it leaves the focus
    /// routing table — what cannot be seen cannot be clicked or focused. Used by
    /// both unmap paths: null attach and `xdg_toplevel.destroy`.
    fn unmap_surface(&mut self, obj: &ObjectId) {
        self.surface_pixels.remove(obj);
        if let Some(&sid) = self.obj_to_surface.get(obj) {
            self.scene.mutate(move |s| s.clear_source(sid));
            self.focus_map.unmap(sid);
            self.refocus_keyboard();
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
    fn refocus_keyboard(&mut self) {
        let target = self.focus_map.topmost();
        if target == self.keyboard_focus {
            return;
        }
        self.keyboard_focus = target;
        let surface = target.and_then(|sid| self.wl_surface_for(sid));
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
    let key = state.alloc_client_key();
    let data = ClientState {
        compositor_state: CompositorClientState::default(),
        key,
        scene: state.scene.clone(),
    };
    let _ = state.dh.insert_client(stream, Arc::new(data));
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
        self.pongs_received.fetch_add(1, Ordering::Relaxed);
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

        // Take the just-committed buffer assignment *and* the accumulated damage
        // out of the surface's *current* state, so each is processed once. We own
        // the buffer release (Smithay would otherwise release the previous buffer
        // only on the next attach — too late for single-buffer clients).
        let obj = surface.id();
        let (assignment, raw_damage) = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let current = guard.current();
            (
                current.buffer.take(),
                std::mem::take(&mut current.damage),
            )
        });

        // The xdg gate: a toplevel may not commit a buffer before acking its
        // initial configure. `ensure_configured` posts the protocol error and
        // returns false, in which case this commit maps nothing — the client is
        // being disconnected anyway. (Smithay posts `xdg_surface.not_constructed`
        // where the spec's dedicated code is `unconfigured_buffer`; the
        // `xdg_surface` object is not reachable through Smithay's public API, so
        // we take its code. Documented in `docs/scene_graph_v1.md` §10.)
        if matches!(assignment, Some(BufferAssignment::NewBuffer(_)))
            && let Some(entry) = self.toplevels.get(&obj)
            && !entry.toplevel.ensure_configured()
        {
            return;
        }

        // Set by the null-attach arm; consumed after the configure check below.
        let mut unmapped = false;

        match assignment {
            Some(BufferAssignment::NewBuffer(buffer)) => {
                // Buffer==surface coordinates in M1 (no scale/transform yet); this
                // is the marked site where the two are merged (M2+ generalizes it).
                let damage_rects = damage_to_rects(&raw_damage);
                let prev = self.surface_pixels.get(&obj).cloned();
                match build_pixel_block(&buffer, prev, &damage_rects) {
                    Ok(Some((block, opaque, content_damage, bytes))) => {
                        self.bytes_copied.fetch_add(bytes, Ordering::Relaxed);
                        self.surface_pixels.insert(obj.clone(), block.clone());
                        let size = (block.width, block.height);
                        let source = TextureSource::Shm(block);
                        // The mapping commit (first content on a toplevel) also
                        // carries the C10 placement; later commits must not touch
                        // geometry, or every frame would damage the whole extent.
                        let placement = match self.toplevels.get_mut(&obj) {
                            Some(entry) if !entry.mapped => {
                                entry.mapped = true;
                                Some(entry.placement)
                            }
                            _ => None,
                        };
                        // Buffer defines pixel size. Only owned `Send` data
                        // crosses to the scene, which computes the frame damage.
                        self.scene.mutate(move |s| {
                            if let Some((dx, dy)) = placement {
                                // Set while the node is still invisible (no source
                                // yet) so this raises no damage of its own; the
                                // attach below damages the mapped extent.
                                s.set_geometry(sid, Transform::Translate { dx, dy }, size);
                            }
                            s.attach_content(sid, size, source, opaque, content_damage);
                        });

                        // Mirror the same fact into the input routing table (T6).
                        // A toplevel's extent is its placement plus the buffer's
                        // size, so this re-runs on every content commit — a client
                        // that commits a differently-sized buffer resizes its
                        // input region with its pixels, in one place, from the
                        // same values the scene was just told about. Non-toplevel
                        // surfaces never enter the table: no role, no pixels, no
                        // input (the T5 rule, extended).
                        if let Some(entry) = self.toplevels.get(&obj) {
                            let (dx, dy) = entry.placement;
                            let rect = Rect::new(dx, dy, size.0 as i32, size.1 as i32);
                            // z is 0 for every toplevel in M1 (stacking policy is
                            // S1's, M4); ties break by SurfaceId in both the
                            // snapshot and the routing table, so input and pixels
                            // agree on who is on top.
                            self.focus_map.map(sid, rect, 0);
                            self.refocus_keyboard();
                        }
                    }
                    // Non-shm buffer or zero-size: nothing to show (no dmabuf in M1).
                    Ok(None) => {}
                    // Access error: Smithay already posted the protocol error /
                    // killed the client; nothing more to do.
                    Err(_) => {}
                }
                buffer.release();
            }
            // Null attach (`wl_surface.attach(null)`): unmap — the node loses its
            // source and becomes invisible; drop the retained block.
            Some(BufferAssignment::Removed) => {
                self.unmap_surface(&obj);
                if let Some(entry) = self.toplevels.get_mut(&obj) {
                    entry.mapped = false;
                    unmapped = true;
                }
            }
            // No buffer change this commit: the node keeps its current pixels.
            // Damage without a new buffer is a no-op — the content did not change,
            // so there is nothing to repaint.
            None => {}
        }

        // The initial configure, sent in response to the client's buffer-less
        // "here I am" commit. Size 0×0 and no states means "you choose" — the core
        // has no size policy to impose (§4 rule 4); the placement it does own is
        // C10's cascade above.
        if let Some(entry) = self.toplevels.get(&obj)
            && !entry.toplevel.is_initial_configure_sent()
        {
            entry.toplevel.send_configure();
        }

        // Re-arm the dance *after* that check, so the unmapping commit itself
        // earns no configure: per xdg-shell an unmapped surface must perform the
        // initial commit/configure sequence again, and the configure belongs to
        // that future commit, not to this one.
        //
        // Smithay would do this re-arming in its own commit hook, but that hook
        // detects unmap through surface state its renderer helpers populate — and
        // we use none of them (we supply our own renderer; that is the seam) — so
        // it is inert for us and the core does it. See `docs/scene_graph_v1.md` §10.
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
        }
    }
}

// `delegate_compositor!` supplies the Dispatch/GlobalDispatch impls for
// wl_compositor/wl_surface/wl_subcompositor/wl_region/wl_callback, routing them
// to `CompositorState` and the `CompositorHandler` impl above.
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
    /// Total pending frame callbacks, republished by the dispatch thread each
    /// pass. Read by [`pending_frame_callbacks`] for the backpressure rig test.
    ///
    /// [`pending_frame_callbacks`]: ProtocolHost::pending_frame_callbacks
    pending_frame_callbacks: Arc<AtomicUsize>,
    /// Running total of buffer bytes copied at commit (damage counter). Read by
    /// [`bytes_copied`](ProtocolHost::bytes_copied).
    bytes_copied: Arc<AtomicUsize>,
    /// `xdg_wm_base.pong` replies received. Read by
    /// [`pongs_received`](ProtocolHost::pongs_received).
    pongs_received: Arc<AtomicUsize>,
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
        let pending = Arc::new(AtomicUsize::new(0));
        let bytes = Arc::new(AtomicUsize::new(0));
        let pongs = Arc::new(AtomicUsize::new(0));

        let presenter = FramePresenter {
            ping,
            ts: present_ts.clone(),
        };
        let pending_for_thread = pending.clone();
        let bytes_for_thread = bytes.clone();
        let pongs_for_thread = pongs.clone();

        let dispatch = std::thread::Builder::new()
            .name("parhelion-proto-0".into())
            .spawn(move || {
                run_dispatch(
                    control_rx,
                    scene,
                    ping_source,
                    present_ts,
                    pending_for_thread,
                    bytes_for_thread,
                    pongs_for_thread,
                )
            })
            .expect("spawn dispatch thread");

        ProtocolHost {
            control_tx,
            dispatch: Some(dispatch),
            presenter,
            pending_frame_callbacks: pending,
            bytes_copied: bytes,
            pongs_received: pongs,
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
        self.pending_frame_callbacks.load(Ordering::Relaxed)
    }

    /// Total buffer bytes copied at commit so far — the damage-tracking counter.
    /// A small-damage commit on a large surface copies far fewer than the whole
    /// buffer (partial copy); the proportionality test asserts on the delta.
    pub fn bytes_copied(&self) -> usize {
        self.bytes_copied.load(Ordering::Relaxed)
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
        self.pongs_received.load(Ordering::Relaxed)
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

/// Dispatch client requests, applying the per-client backpressure bound, on the
/// `Display` that calloop's `Generic` source is guarding. **Does not flush** —
/// flushing is owned by the single site in [`run_dispatch`]; this only reads
/// requests (firing the `CompositorHandler` callbacks).
///
/// Backpressure (I-10): rather than `dispatch_clients` (which reads every ready
/// client), we dispatch **per client** and skip any whose pending frame-callback
/// backlog is at or over [`MAX_PENDING_FRAME_CALLBACKS`]. A skipped client's
/// socket is simply not read this pass, so its requests stay in the kernel
/// buffer (and its own writes block once that fills) until a render tick drains
/// its callbacks below the bound — never dropped, and shard-mates keep being
/// served. Note (v1 cost, `docs/scene_graph_v1.md` §8): while a skipped client
/// has unread data the level-triggered `Display` source stays ready, so the loop
/// spins to keep serving others during an active flood; this is the dispatch
/// thread, not the frame path (I-1 is unaffected). The tighter fix is M2.
///
/// `unsafe` is confined here and justified at the block. The workspace lints
/// warn on `unsafe_code`; this one use is the documented Smithay + calloop
/// integration and is allowed with the justification below.
#[allow(unsafe_code)]
fn pump_display(
    display: &mut NoIoDrop<Display<State>>,
    state: &mut State,
) -> std::io::Result<PostAction> {
    // SAFETY: `NoIoDrop::get_mut` is unsafe solely because using the returned
    // `&mut` to drop or close the underlying fd would corrupt calloop's
    // registration of this source. We only call `backend()`/`dispatch_single_client`,
    // which read requests — never drop or close the fd. This is the standard
    // wayland-server-on-calloop pattern (cf. Smithay's anvil).
    let display = unsafe { display.get_mut() };

    // Drop object handles for surfaces whose client is gone, so the backlog and
    // callback bookkeeping never touches a dead resource; drop their retained
    // pixel blocks too (a client that disconnects without destroying surfaces).
    state.surfaces.retain(|_, s| s.is_alive());
    state
        .surface_pixels
        .retain(|obj, _| state.surfaces.contains_key(obj));

    // Per-client pending-callback backlog, computed once for the throttle
    // decision below (and republished for observability).
    let mut backlog: HashMap<ClientId, usize> = HashMap::new();
    for surface in state.surfaces.values() {
        if let Some(client) = surface.client() {
            *backlog.entry(client.id()).or_default() += surface_frame_backlog(surface);
        }
    }

    // Dispatch each live client, skipping the over-bound ones (their sockets go
    // unread — the backpressure). Collect ids first so the immutable backend
    // handle borrow ends before the mutable per-client dispatch.
    let ids: Vec<ClientId> = {
        let mut v = Vec::new();
        state.dh.backend_handle().with_all_clients(|id| v.push(id));
        v
    };
    for id in ids {
        let over = backlog.get(&id).copied().unwrap_or(0) >= MAX_PENDING_FRAME_CALLBACKS;
        if !over {
            // Ignore per-client errors (e.g. WouldBlock when a client has nothing
            // pending, or a client that just disconnected): they are not fatal to
            // the loop, and disconnect is handled via ClientData::disconnected.
            let _ = display.backend().dispatch_single_client(state, id);
        }
    }

    // Republish the up-to-date total *after* dispatch, so a caller that just
    // round-tripped a client observes that client's new backlog (the rig test
    // relies on this to know when the bound is reached).
    let total: usize = state
        .surfaces
        .values()
        .map(surface_frame_backlog)
        .sum();
    state
        .pending_frame_callbacks
        .store(total, Ordering::Relaxed);

    Ok(PostAction::Continue)
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
    state.pending_frame_callbacks.store(0, Ordering::Relaxed);
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
    pending_frame_callbacks: Arc<AtomicUsize>,
    bytes_copied: Arc<AtomicUsize>,
    pongs_received: Arc<AtomicUsize>,
) {
    let display: Display<State> = Display::new().expect("create wayland display");
    let dh = display.handle();

    // The loop is created before the state because the state holds a handle to
    // it: sources that arrive *after* the loop is running (the listening socket,
    // T6) are registered from inside a source callback, which needs the handle.
    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("create calloop event loop");
    let handle = event_loop.handle();
    let compositor_state = CompositorState::new::<State>(&dh);
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
    // `SeatState` owns the seat (it keeps the list of them alive); we keep only
    // the two capability handles, which is all the input path needs.
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

    let mut state = State {
        compositor_state,
        shm_state,
        xdg_shell_state,
        seat_state,
        keyboard,
        pointer,
        pointer_pos: (0.0, 0.0),
        focus_map: FocusMap::new(),
        keyboard_focus: None,
        dh: dh.clone(),
        loop_handle: handle.clone(),
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
        pending_frame_callbacks,
        bytes_copied,
        pongs_received,
        stop: false,
    };

    // The Display's poll fd: readable when a client has pending requests. The
    // Generic source *owns* the Display, handing it back as the callback's
    // second argument so we can dispatch and flush in place.
    handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_readiness, display, state: &mut State| pump_display(display, state),
        )
        .expect("register display source");

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
        })
        .expect("register frame-present ping source");

    // A modest timeout keeps the loop responsive to the stop flag and re-checks
    // level-triggered readiness — it is a safety net, not the primary wakeup
    // (all sources wake the loop).
    while !state.stop {
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
