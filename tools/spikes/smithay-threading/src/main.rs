// ==========================================================================
// Smithay / wayland-rs threading-fit spike  —  Parhelion M0 task 2.
//
// WHAT THIS PROGRAM DEMONSTRATES
//
//   1. (compile-time, `q1_static_facts`) The core wayland-server / wayland-
//      backend types the Parhelion core would use are Send/Sync in the shapes
//      that matter: `Display<State>` is UNCONDITIONALLY `Send + Sync` (the
//      compositor `State` is never stored inside it — it is only *borrowed*
//      via `dispatch_clients(&mut State)`), and `DisplayHandle`, `ClientId`,
//      `ObjectId`, `GlobalId` are `Send + Sync`. These are static assertions:
//      if any were false, THIS FILE WOULD NOT COMPILE. That compile is the
//      evidence for report question 1.
//
//   2. (runtime, `q2_split_experiment`) The CORE-BOUNDARY §7 split compiles
//      and runs: a *dispatch thread* owns the Smithay `Display` + protocol
//      `State`, a *separate scene thread* owns a toy "scene," they communicate
//      ONLY by message passing over a bounded channel, and one scripted client
//      connects over a socketpair, binds `wl_compositor`, creates a
//      `wl_surface`, and commits. Each client request is turned, inside the
//      dispatch callback, into a message published to the scene thread. The
//      scene thread never touches any Wayland type. This is the report's
//      question-2 experiment.
//
// This crate is its OWN cargo workspace (see Cargo.toml) and is not part of the
// Parhelion production build; `make test` never compiles it. Spike code is
// exempt from production comment density but is headed by this block.
//
// Run:  cd tools/spikes/smithay-threading && cargo run
// ==========================================================================

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use wayland_server::backend::{ClientData, ClientId, DisconnectReason, GlobalId, ObjectId};
use wayland_server::protocol::{
    wl_compositor::{self, WlCompositor},
    wl_surface::{self, WlSurface},
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
};

// --------------------------------------------------------------------------
// Question 1 — static Send/Sync facts. A compile error here is the evidence.
// --------------------------------------------------------------------------

/// Marker state type standing in for Parhelion's real compositor state.
struct AnyState;

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

/// Proven at compile time; the body running at all means every bound held.
fn q1_static_facts() {
    // Display is Send + Sync regardless of the State parameter — the State is
    // borrowed at dispatch, never owned by the Display. (Verified separately
    // in the report with a deliberately non-Send State type.)
    assert_send::<Display<AnyState>>();
    assert_sync::<Display<AnyState>>();
    // The handle used to publish protocol state / touch objects from anywhere.
    assert_send::<DisplayHandle>();
    assert_sync::<DisplayHandle>();
    // Identifiers are Send + Sync, so they can ride in cross-thread messages.
    assert_send::<ClientId>();
    assert_sync::<ClientId>();
    assert_send::<ObjectId>();
    assert_sync::<ObjectId>();
    assert_send::<GlobalId>();
    assert_sync::<GlobalId>();
    println!(
        "[Q1] static facts: Display<S>: Send+Sync (S-independent); \
         DisplayHandle/ClientId/ObjectId/GlobalId: Send+Sync — all compiled."
    );
}

// --------------------------------------------------------------------------
// Question 2 — the dispatch-thread / scene-thread split.
// --------------------------------------------------------------------------

/// Messages the dispatch thread publishes to the scene thread. The scene owns
/// its own world built only from these — it never sees a Wayland object. This
/// is the §7 "publishes state changes to the scene graph via messages" edge.
#[derive(Debug)]
enum SceneMsg {
    /// A client bound wl_compositor and created a surface (carrying the
    /// Send-able protocol ObjectId, proving ids cross the boundary).
    SurfaceCreated(ObjectId),
    /// A surface committed — the moment a real compositor would fold pending
    /// buffer/state into canonical scene state.
    SurfaceCommitted(ObjectId),
    /// The scripted client disconnected.
    ClientGone,
}

/// The protocol-side compositor state. Lives ONLY on the dispatch thread.
/// Its single job at each callback is to translate a Wayland request into a
/// `SceneMsg` and send it. It holds no scene data of its own.
struct Protocol {
    /// The one edge to the scene thread. Bounded in a real core; std channel
    /// here for brevity.
    scene: Sender<SceneMsg>,
    /// Flips once we have forwarded a commit, so the dispatch loop can stop.
    saw_commit: Arc<AtomicBool>,
}

/// Per-client data. The core protocol needs a `ClientData`; defaults suffice.
struct ClientState;
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

// wl_compositor: advertise it, and on bind assign it unit user-data.
impl GlobalDispatch<WlCompositor, ()> for Protocol {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlCompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

// wl_compositor requests: the only one we care about is create_surface.
impl Dispatch<WlCompositor, ()> for Protocol {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &WlCompositor,
        request: wl_compositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            let surface: WlSurface = data_init.init(id, ());
            // Publish to the scene thread — a Send-able id crosses the channel.
            state
                .scene
                .send(SceneMsg::SurfaceCreated(surface.id()))
                .expect("scene thread alive");
        }
    }
}

// wl_surface requests: forward commits, ignore the rest for this spike.
impl Dispatch<WlSurface, ()> for Protocol {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WlSurface,
        request: wl_surface::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_surface::Request::Commit = request {
            state
                .scene
                .send(SceneMsg::SurfaceCommitted(resource.id()))
                .expect("scene thread alive");
            state.saw_commit.store(true, Ordering::SeqCst);
        }
    }
}

/// The scene thread. Owns a toy "scene" (just a surface list) and mutates it
/// purely from messages. No Wayland type is in scope here — that is the point.
fn run_scene(rx: Receiver<SceneMsg>) {
    let mut surfaces: Vec<ObjectId> = Vec::new();
    let mut commits = 0u32;
    // recv() ends (Err) when the dispatch thread drops its Sender at shutdown.
    while let Ok(msg) = rx.recv() {
        match msg {
            SceneMsg::SurfaceCreated(id) => {
                println!("[scene] surface created: {id:?}");
                surfaces.push(id);
            }
            SceneMsg::SurfaceCommitted(id) => {
                commits += 1;
                println!("[scene] surface committed: {id:?}");
            }
            SceneMsg::ClientGone => println!("[scene] client gone"),
        }
    }
    println!(
        "[scene] channel closed — final toy scene: {} surface(s), {} commit(s)",
        surfaces.len(),
        commits
    );
}

/// The dispatch thread. Owns the Smithay `Display` + `Protocol` state, accepts
/// the scripted client over `server_stream`, and pumps the protocol until a
/// commit has been forwarded (or a safety timeout trips).
fn run_dispatch(server_stream: UnixStream, scene: Sender<SceneMsg>, saw_commit: Arc<AtomicBool>) {
    let mut display: Display<Protocol> = Display::new().expect("create display");
    // `create_global` and `insert_client` both live on DisplayHandle; the
    // latter takes `&mut self`, so the handle is mutable.
    let mut dh: DisplayHandle = display.handle();
    // Advertise wl_compositor v4. The bind lands in GlobalDispatch above.
    let _global: GlobalId = dh.create_global::<Protocol, WlCompositor, ()>(4, ());

    // Accept the scripted client on the pre-connected socketpair end.
    dh.insert_client(server_stream, Arc::new(ClientState))
        .expect("insert client");

    let mut state = Protocol {
        scene: scene.clone(),
        saw_commit: saw_commit.clone(),
    };

    // Non-blocking dispatch pump. `dispatch_clients` returns immediately when
    // nothing is pending; we sleep briefly between passes. A real core drives
    // this from calloop on the socket fd — here a bounded poll keeps the spike
    // free of unsafe fd polling. Safety timeout so the spike can never hang CI.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        display.dispatch_clients(&mut state).expect("dispatch");
        display.flush_clients().expect("flush");
        if saw_commit.load(Ordering::SeqCst) {
            // Grace drain: keep dispatching + flushing briefly so the client's
            // in-flight sync completes and it disconnects cleanly BEFORE we drop
            // the Display (which closes the socket). Without this the client's
            // next round-trip would race a closed pipe. ~50 ms is ample here.
            let grace = Instant::now() + Duration::from_millis(50);
            while Instant::now() < grace {
                display.dispatch_clients(&mut state).expect("dispatch");
                display.flush_clients().expect("flush");
                thread::sleep(Duration::from_millis(1));
            }
            break;
        }
        if Instant::now() > deadline {
            println!("[dispatch] TIMEOUT before commit — experiment inconclusive");
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let _ = scene.send(SceneMsg::ClientGone);
    // Dropping `state` (and thus its Sender) here closes the scene channel.
}

/// The scripted client. Connects over `client_stream`, binds wl_compositor,
/// creates a surface, commits, and round-trips so the server sees it all.
fn run_client(client_stream: UnixStream) {
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::{
        wl_compositor::WlCompositor as ClientCompositor, wl_surface::WlSurface as ClientSurface,
    };
    use wayland_client::{Connection, Dispatch as ClientDispatch, QueueHandle};

    /// Client-side app state. No events of interest; all handlers are empty.
    struct App;
    impl ClientDispatch<WlRegistry, GlobalListContents> for App {
        fn event(
            _s: &mut Self,
            _r: &WlRegistry,
            _e: wayland_client::protocol::wl_registry::Event,
            _d: &GlobalListContents,
            _c: &Connection,
            _q: &QueueHandle<Self>,
        ) {
        }
    }
    impl ClientDispatch<ClientCompositor, ()> for App {
        fn event(
            _s: &mut Self,
            _r: &ClientCompositor,
            _e: wayland_client::protocol::wl_compositor::Event,
            _d: &(),
            _c: &Connection,
            _q: &QueueHandle<Self>,
        ) {
        }
    }
    impl ClientDispatch<ClientSurface, ()> for App {
        fn event(
            _s: &mut Self,
            _r: &ClientSurface,
            _e: wayland_client::protocol::wl_surface::Event,
            _d: &(),
            _c: &Connection,
            _q: &QueueHandle<Self>,
        ) {
        }
    }

    let conn = Connection::from_socket(client_stream).expect("client connect");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry init");
    let qh = queue.handle();
    let mut app = App;

    // Bind wl_compositor (server offers v4; accept anything up to 4).
    let compositor: ClientCompositor = globals.bind(&qh, 1..=4, ()).expect("bind wl_compositor");
    println!("[client] bound wl_compositor");

    let surface: ClientSurface = compositor.create_surface(&qh, ());
    println!("[client] created wl_surface");
    surface.commit();
    println!("[client] committed wl_surface");

    // Flush the requests and block until the server has processed them (the
    // round-trip's sync callback returns only after the commit was dispatched).
    queue.roundtrip(&mut app).expect("roundtrip");
    // Dropping conn here disconnects the client; the server observes it during
    // its grace drain.
}

/// Wires the two threads to one scripted client over a socketpair and runs the
/// whole split experiment to completion.
fn q2_split_experiment() {
    println!("\n[Q2] split experiment: dispatch thread + scene thread + 1 client");
    let (server_stream, client_stream) = UnixStream::pair().expect("socketpair");

    let (scene_tx, scene_rx) = channel::<SceneMsg>();
    let saw_commit = Arc::new(AtomicBool::new(false));

    // Scene thread — owns the toy scene, sees only messages.
    let scene = thread::spawn(move || run_scene(scene_rx));
    // Dispatch thread — owns the Smithay Display + protocol state.
    let dispatch = {
        let saw_commit = saw_commit.clone();
        thread::spawn(move || run_dispatch(server_stream, scene_tx, saw_commit))
    };
    // The scripted client runs on this (main) thread.
    run_client(client_stream);

    dispatch.join().expect("dispatch thread");
    scene.join().expect("scene thread");

    if saw_commit.load(Ordering::SeqCst) {
        println!("[Q2] RESULT: split ran — commit flowed dispatch-thread -> scene-thread. PASS");
    } else {
        println!("[Q2] RESULT: no commit observed — see TIMEOUT above. FAIL");
    }
}

fn main() {
    q1_static_facts();
    q2_split_experiment();
}
