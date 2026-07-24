//! `ProtocolHost` — the Wayland protocol frontend, at `shards = 1`.
//!
//! Governing design: `docs/CORE-BOUNDARY.md` §3 (C3: protocol machinery is
//! in-core) and §7 (T-proto[n] owns client sockets + protocol object state and
//! publishes changes to the scene by message). Governing decision: the three
//! entries under "2026-07-24 — Smithay threading fit" in the decision log —
//! this module implements their four interface requirements. It is governed by
//! CORE-BOUNDARY §3/§7 until it earns its own document (subsystem table).
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
//!    `Display`; everything it tells the world is a [`LedgerMsg`] carrying
//!    core-assigned [`SurfaceId`]/[`ClientKey`], never a borrowed `WlSurface`.
//! 3. **The receiving side is the minimal [`crate::ledger::Ledger`].** Owned
//!    here on the consumer side; [`ProtocolHost::sync`] folds pending messages
//!    into it. In production the consumer is the scene-graph thread; here the
//!    harness pumps it. Either way the edge is one-directional and async (I-3).
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
//! messages (admit client / shut down) into the thread.
//!
//! # Protocol scope (M0)
//!
//! `wl_compositor` only: surface create / commit / destroy, via
//! `smithay::wayland::compositor` (the frontend layer the decision points at).
//! xdg-shell, buffers, input, and frame callbacks are M1.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use smithay::reexports::calloop::{
    channel::{channel as calloop_channel, Channel, Event as ChannelEvent, Sender as CalloopSender},
    generic::{Generic, NoIoDrop},
    EventLoop, Interest, Mode, PostAction,
};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState,
};

use crate::ledger::{ClientKey, Ledger, LedgerMsg, SurfaceId};

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

/// Guards the same decision's requirement 2: the tokens that cross the
/// dispatch→scene channel are `Send`, so they may ride a cross-thread message.
const _GUARD_TOKENS_SEND: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<SurfaceId>();
    assert_send::<ClientKey>();
    assert_send::<LedgerMsg>();
};

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
/// automatically on disconnect. Holds the Smithay compositor client state and
/// the core-assigned [`ClientKey`] used for ledger attribution.
struct ClientState {
    /// Required by `smithay::wayland::compositor` (per-client protocol state).
    compositor_state: CompositorClientState,
    /// This client's core-assigned key.
    key: ClientKey,
    /// Channel to the ledger, so disconnect can publish `ClientGone`.
    ledger_tx: mpsc::Sender<LedgerMsg>,
}

impl ClientData for ClientState {
    /// On disconnect, publish `ClientGone` so the ledger drops this client's
    /// surfaces. Runs on the dispatch thread during `dispatch_clients`.
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        // Best-effort: if the host is being torn down the receiver may be gone.
        let _ = self.ledger_tx.send(LedgerMsg::ClientGone { client: self.key });
    }
}

/// The dispatch shard's protocol state — thin, protocol-only. Owns no scene
/// data: its whole job is to translate protocol callbacks into [`LedgerMsg`]s.
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
    /// The publish edge to the ledger owner.
    ledger_tx: mpsc::Sender<LedgerMsg>,
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

        // The owning client's core key lives in its ClientState.
        let client = surface.client().expect("surface has a live client");
        let key = client
            .get_data::<ClientState>()
            .expect("admitted client has ClientState")
            .key;

        let _ = self.ledger_tx.send(LedgerMsg::SurfaceCreated {
            surface: sid,
            client: key,
        });
    }

    /// A surface committed: publish `SurfaceCommitted` for its [`SurfaceId`].
    fn commit(&mut self, surface: &WlSurface) {
        if let Some(&sid) = self.obj_to_surface.get(&surface.id()) {
            let _ = self.ledger_tx.send(LedgerMsg::SurfaceCommitted { surface: sid });
        }
    }

    /// A surface was destroyed: drop the mapping and publish `SurfaceDestroyed`.
    fn destroyed(&mut self, surface: &WlSurface) {
        if let Some(sid) = self.obj_to_surface.remove(&surface.id()) {
            let _ = self.ledger_tx.send(LedgerMsg::SurfaceDestroyed { surface: sid });
        }
    }
}

// `delegate_compositor!` supplies the Dispatch/GlobalDispatch impls for
// wl_compositor/wl_surface/wl_subcompositor/wl_region/wl_callback, routing them
// to `CompositorState` and the `CompositorHandler` impl above.
smithay::delegate_compositor!(State);

// ==========================================================================
// The ProtocolHost handle (owner of the consumer side).
// ==========================================================================

/// The protocol frontend, seen from the rest of the core (and the harness).
///
/// Owns the consumer side of the dispatch→scene edge: the [`Ledger`] and the
/// receiver it is folded from, plus the control channel and join handle for the
/// dispatch thread. Dropping it shuts the thread down cleanly.
pub struct ProtocolHost {
    /// Control channel into the dispatch thread (admit client / shutdown).
    control_tx: CalloopSender<Control>,
    /// Incoming ledger messages from the dispatch thread.
    ledger_rx: mpsc::Receiver<LedgerMsg>,
    /// The consumer-side ledger, folded from `ledger_rx` by [`Self::sync`].
    ledger: Ledger,
    /// The dispatch thread, joined on drop.
    dispatch: Option<JoinHandle<()>>,
}

impl ProtocolHost {
    /// Start a `shards = 1` protocol host: spin up the dispatch thread (which
    /// creates the `Display`, advertises `wl_compositor`, and runs the calloop
    /// loop) and return a handle to its consumer side.
    pub fn new() -> Self {
        let (control_tx, control_rx) = calloop_channel::<Control>();
        let (ledger_tx, ledger_rx) = mpsc::channel::<LedgerMsg>();

        let dispatch = std::thread::Builder::new()
            .name("parhelion-proto-0".into())
            .spawn(move || run_dispatch(control_rx, ledger_tx))
            .expect("spawn dispatch thread");

        ProtocolHost {
            control_tx,
            ledger_rx,
            ledger: Ledger::new(),
            dispatch: Some(dispatch),
        }
    }

    /// The accept seam (requirement 1). Admit a client connected on `stream` —
    /// an external `ListeningSocket::accept` loop and the test rig both call
    /// this; it routes the stream to a shard (the only shard, at `shards = 1`).
    pub fn add_client(&self, stream: UnixStream) {
        // If the dispatch thread is gone the host is being torn down; drop it.
        let _ = self.control_tx.send(Control::AddClient(stream));
    }

    /// Fold all currently-available ledger messages into the ledger, without
    /// blocking. Use after a client round-trip, when protocol ordering
    /// guarantees the messages of interest are already enqueued.
    pub fn sync(&mut self) {
        while let Ok(msg) = self.ledger_rx.try_recv() {
            self.ledger.apply(msg);
        }
    }

    /// Fold messages until `pred` holds on the ledger or `timeout` elapses.
    /// Returns whether `pred` was satisfied. Used where there is no round-trip
    /// to synchronize on (e.g. observing a client disconnect), so the wait is
    /// on a definite condition rather than a fixed sleep.
    pub fn wait_until(&mut self, timeout: Duration, pred: impl Fn(&Ledger) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        self.sync();
        while !pred(&self.ledger) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return pred(&self.ledger);
            }
            match self.ledger_rx.recv_timeout(remaining) {
                Ok(msg) => self.ledger.apply(msg),
                Err(_) => return pred(&self.ledger),
            }
        }
        true
    }

    /// Read-only view of the ledger for assertions.
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }
}

impl Default for ProtocolHost {
    fn default() -> Self {
        Self::new()
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

/// Dispatch queued client requests (firing the `CompositorHandler` callbacks)
/// then flush replies back out, on the `Display` that calloop's `Generic` source
/// is guarding.
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
    // registration of this source. We only call `dispatch_clients` and
    // `flush_clients`, neither of which drops or closes the fd — they read
    // requests and write replies. This is the standard wayland-server-on-calloop
    // pattern (cf. Smithay's anvil).
    let display = unsafe { display.get_mut() };
    display.dispatch_clients(state)?;
    display.flush_clients()?;
    Ok(PostAction::Continue)
}

/// Run the shard-0 dispatch loop: own the `Display`, advertise `wl_compositor`,
/// and pump calloop until a `Shutdown` control message arrives.
fn run_dispatch(control_rx: Channel<Control>, ledger_tx: mpsc::Sender<LedgerMsg>) {
    let display: Display<State> = Display::new().expect("create wayland display");
    let dh = display.handle();
    let compositor_state = CompositorState::new::<State>(&dh);

    let mut state = State {
        compositor_state,
        dh: dh.clone(),
        next_surface_id: 0,
        next_client_key: 0,
        obj_to_surface: HashMap::new(),
        ledger_tx: ledger_tx.clone(),
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

    // Control channel: admit clients (the accept seam) and shutdown.
    handle
        .insert_source(control_rx, move |event, _, state: &mut State| {
            if let ChannelEvent::Msg(control) = event {
                match control {
                    Control::AddClient(stream) => {
                        let key = state.alloc_client_key();
                        let data = ClientState {
                            compositor_state: CompositorClientState::default(),
                            key,
                            ledger_tx: ledger_tx.clone(),
                        };
                        // Insert on this thread's Display — the assignment seam.
                        let _ = state.dh.insert_client(stream, std::sync::Arc::new(data));
                    }
                    Control::Shutdown => state.stop = true,
                }
            }
        })
        .expect("register control source");

    // A modest timeout keeps the loop responsive to the stop flag and re-checks
    // level-triggered readiness — it is a safety net, not the primary wakeup
    // (both sources wake the loop). Flushing on client insertion happens on the
    // next dispatch pass, which the buffered client data triggers.
    while !state.stop {
        event_loop
            .dispatch(Some(Duration::from_millis(20)), &mut state)
            .expect("calloop dispatch");
    }
}
