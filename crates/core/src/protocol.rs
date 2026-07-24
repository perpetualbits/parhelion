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
//! # Protocol scope (M0/M1 T1–T2)
//!
//! `wl_compositor` only: surface create / commit / destroy, plus
//! `wl_surface.frame` callbacks (T2), via `smithay::wayland::compositor` (the
//! frontend layer the decision points at). Buffers (T3), xdg-shell (T5), and
//! input (T6) are later.

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
    EventLoop, Interest, Mode, PostAction,
};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_callback::WlCallback;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::wayland::compositor::{
    with_states, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
};

use crate::scene::{ClientKey, ProtocolEvent, SceneHandle, SurfaceId};

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

/// The dispatch shard's protocol state — thin, protocol-only. Owns no scene
/// data: its whole job is to translate protocol callbacks into [`ProtocolEvent`]s
/// published to the scene owner.
struct State {
    /// Smithay's compositor global/handler state.
    compositor_state: CompositorState,
    /// Handle used to admit clients and resolve object→client.
    dh: DisplayHandle,
    /// Monotonic source of [`SurfaceId`]s.
    next_surface_id: u64,
    /// Monotonic source of [`ClientKey`]s.
    next_client_key: u64,
    /// Maps live protocol surfaces to their core id, for commit/destroy lookup.
    obj_to_surface: HashMap<ObjectId, SurfaceId>,
    /// Live `wl_surface` handles, keyed by object id — the set [`present`] drains
    /// frame callbacks from, and the set the per-client backlog is computed over.
    /// The `WlSurface` never leaves this thread (it is not `Send`), which is the
    /// whole point: protocol objects stay on the dispatch thread (§7).
    surfaces: HashMap<ObjectId, WlSurface>,
    /// The publish edge to the scene owner.
    scene: SceneHandle,
    /// Latest presentation timestamp from the render side (shared with the
    /// [`FramePresenter`]); read by [`present`] when firing callbacks.
    present_ts: Arc<AtomicU32>,
    /// Total pending frame callbacks across all clients, republished each pass
    /// for observability (`ProtocolHost::pending_frame_callbacks`, used by the
    /// backpressure rig test). Not load-bearing for dispatch.
    pending_frame_callbacks: Arc<AtomicUsize>,
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

    /// A surface committed: publish `SurfaceCommitted` for its [`SurfaceId`].
    fn commit(&mut self, surface: &WlSurface) {
        if let Some(&sid) = self.obj_to_surface.get(&surface.id()) {
            self.scene.emit(ProtocolEvent::SurfaceCommitted { surface: sid });
        }
    }

    /// A surface was destroyed: drop the mappings and publish `SurfaceDestroyed`.
    fn destroyed(&mut self, surface: &WlSurface) {
        self.surfaces.remove(&surface.id());
        if let Some(sid) = self.obj_to_surface.remove(&surface.id()) {
            self.scene.emit(ProtocolEvent::SurfaceDestroyed { surface: sid });
        }
    }
}

// `delegate_compositor!` supplies the Dispatch/GlobalDispatch impls for
// wl_compositor/wl_surface/wl_subcompositor/wl_region/wl_callback, routing them
// to `CompositorState` and the `CompositorHandler` impl above.
smithay::delegate_compositor!(State);

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

        let presenter = FramePresenter {
            ping,
            ts: present_ts.clone(),
        };
        let pending_for_thread = pending.clone();

        let dispatch = std::thread::Builder::new()
            .name("parhelion-proto-0".into())
            .spawn(move || {
                run_dispatch(control_rx, scene, ping_source, present_ts, pending_for_thread)
            })
            .expect("spawn dispatch thread");

        ProtocolHost {
            control_tx,
            dispatch: Some(dispatch),
            presenter,
            pending_frame_callbacks: pending,
        }
    }

    /// The accept seam (requirement 1). Admit a client connected on `stream` —
    /// an external `ListeningSocket::accept` loop and the test rig both call
    /// this; it routes the stream to a shard (the only shard, at `shards = 1`).
    pub fn add_client(&self, stream: UnixStream) {
        // If the dispatch thread is gone the host is being torn down; drop it.
        let _ = self.control_tx.send(Control::AddClient(stream));
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
    // callback bookkeeping never touches a dead resource.
    state.surfaces.retain(|_, s| s.is_alive());

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
) {
    let display: Display<State> = Display::new().expect("create wayland display");
    let dh = display.handle();
    let compositor_state = CompositorState::new::<State>(&dh);

    let mut state = State {
        compositor_state,
        dh: dh.clone(),
        next_surface_id: 0,
        next_client_key: 0,
        obj_to_surface: HashMap::new(),
        surfaces: HashMap::new(),
        scene: scene.clone(),
        present_ts,
        pending_frame_callbacks,
        stop: false,
    };

    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("create calloop event loop");
    let handle = event_loop.handle();

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
                    Control::AddClient(stream) => {
                        let key = state.alloc_client_key();
                        let data = ClientState {
                            compositor_state: CompositorClientState::default(),
                            key,
                            scene: state.scene.clone(),
                        };
                        // Insert on this thread's Display — the assignment seam.
                        let _ = state.dh.insert_client(stream, std::sync::Arc::new(data));
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
