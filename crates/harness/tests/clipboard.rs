//! T7b clipboard tests: the selection, end to end, between real clients.
//!
//! Governing design: `docs/scene_graph_v1.md` §12.2 and the decision-log entry
//! "Clipboard v1 = core-protocol selection semantics, focus-gated". `foot`
//! refuses to start without `wl_data_device_manager`, which is how the clipboard
//! stopped being optional; these tests are what "implemented, not stubbed" means
//! in practice.
//!
//! # What is actually being tested
//!
//! The bytes never pass through the compositor. A copy publishes a *source*, a
//! paste asks the *offer* for a pipe, and the two clients transfer through it
//! directly — the compositor brokers the introduction and gets out of the way.
//! So these tests drive both ends and read the bytes that come out the far side.
//!
//! **The focus gate is asserted, not assumed** (client C): only the client with
//! keyboard focus may set the selection, which is the protocol's own answer to
//! "who may overwrite what the user copied" and Parhelion's v1 capability model
//! (I-7's letter; the deeper design is M4's).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_core::render::Compositor;

use parhelion_core::input::InputEvent;
use parhelion_core::protocol::ProtocolHost;
use parhelion_core::scene::{SceneThread, SurfaceId};
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

const W: i32 = 16;
const H: i32 = 16;
const MIME: &str = "text/plain";

fn white() -> Vec<u8> {
    vec![255u8; (W * H * 4) as usize]
}

/// A scene, a host, and `n` connected clients.
fn fixture(n: usize) -> (SceneThread, ProtocolHost, Vec<ScriptedClient>) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let clients = (0..n)
        .map(|_| {
            let (server_end, client_end) = UnixStream::pair().expect("socketpair");
            host.add_client(server_end);
            ScriptedClient::connect(client_end)
        })
        .collect();
    (scene, host, clients)
}

/// Give `client` a mapped window (which takes keyboard focus, being topmost) and
/// an input event, so it holds a serial it may set the selection with.
fn focus_with_input(host: &ProtocolHost, client: &mut ScriptedClient, time_ms: u32) {
    let _win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white());
    client.pump_until_input_events(1);
    // KEY_A down/up: a real input event, so the client has a real serial.
    host.input(InputEvent::Key {
        code: 30,
        pressed: true,
        time_ms,
    });
    host.input(InputEvent::Key {
        code: 30,
        pressed: false,
        time_ms: time_ms + 1,
    });
    client.pump_until_input_events(3);
}

/// Wait until `client` has seen at least `n` selection events.
fn pump_until_selections(client: &mut ScriptedClient, n: u32) {
    for _ in 0..1000 {
        if client.selection_event_count() >= n {
            return;
        }
        client.roundtrip();
    }
    panic!(
        "expected {n} selection events, saw {}",
        client.selection_event_count()
    );
}

/// The whole point: A copies, focus moves to B, B is offered the clipboard and
/// reads **the exact bytes** through the pipe.
#[test]
fn a_copies_and_the_next_focused_client_pastes_the_exact_bytes() {
    let (_scene, host, mut clients) = fixture(2);
    let (mut b, mut a) = (clients.pop().unwrap(), clients.pop().unwrap());

    // A takes focus and copies.
    focus_with_input(&host, &mut a, 10);
    let _source = a.set_clipboard(MIME, b"parhelion clipboard v1");
    a.roundtrip();

    // B maps on top, taking focus — and with it, the clipboard offer.
    focus_with_input(&host, &mut b, 20);
    pump_until_selections(&mut b, 1);

    assert!(
        b.has_clipboard_offer(),
        "the newly focused client was offered the clipboard"
    );
    assert_eq!(
        b.clipboard_mimes(),
        &[MIME.to_string()],
        "with the mime type A published"
    );

    let bytes = b.read_clipboard(MIME, &mut [&mut a]);
    assert_eq!(
        bytes, b"parhelion clipboard v1",
        "the bytes crossed from A to B through the pipe, unaltered"
    );
}

/// **The focus gate, asserted.** An unfocused client's attempt to set the
/// selection is refused, so it cannot overwrite what the user copied — and an
/// unfocused client is never handed the offer either.
#[test]
fn an_unfocused_client_can_neither_set_nor_receive_the_selection() {
    let (_scene, host, mut clients) = fixture(3);
    let (mut c, mut b, mut a) = (
        clients.pop().unwrap(),
        clients.pop().unwrap(),
        clients.pop().unwrap(),
    );

    // A is focused and copies.
    focus_with_input(&host, &mut a, 10);
    let _source_a = a.set_clipboard(MIME, b"from A");
    a.roundtrip();

    // C connects but never maps a window: no focus, no serial, no offer.
    c.roundtrip();
    c.roundtrip();
    assert!(
        !c.has_clipboard_offer(),
        "an unfocused client is not handed the clipboard"
    );
    assert_eq!(
        c.selection_event_count(),
        0,
        "and hears nothing about it at all"
    );

    // C tries to set the selection anyway, with the only serial it has (none).
    let _source_c = c.set_clipboard(MIME, b"from C: should not take");
    c.roundtrip();
    c.roundtrip();

    // B now takes focus. It must receive A's clipboard, not C's.
    focus_with_input(&host, &mut b, 20);
    pump_until_selections(&mut b, 1);
    let bytes = b.read_clipboard(MIME, &mut [&mut a, &mut c]);
    assert_eq!(
        bytes, b"from A",
        "the unfocused client's set_selection did not take — the clipboard is still A's"
    );
}

/// Replacing the selection cancels the previous source, which is how a client
/// knows to stop holding the data it published.
#[test]
fn replacing_the_selection_cancels_the_previous_source() {
    let (_scene, host, mut clients) = fixture(1);
    let mut a = clients.pop().unwrap();

    focus_with_input(&host, &mut a, 10);
    let _first = a.set_clipboard(MIME, b"first");
    a.roundtrip();
    assert_eq!(a.source_cancelled_count(), 0, "nothing cancelled yet");

    // The same client copies again: the first source is superseded.
    let _second = a.set_clipboard(MIME, b"second");
    for _ in 0..1000 {
        if a.source_cancelled_count() > 0 {
            break;
        }
        a.roundtrip();
    }
    assert_eq!(
        a.source_cancelled_count(),
        1,
        "the replaced source was cancelled, exactly once"
    );
}

/// When the client that owns the clipboard dies, the clipboard goes with it —
/// and the surviving client is told, rather than being left holding an offer
/// nobody can answer.
#[test]
fn the_owners_death_clears_the_selection_without_disturbing_others() {
    let (scene, host, mut clients) = fixture(2);
    let (mut b, mut a) = (clients.pop().unwrap(), clients.pop().unwrap());

    focus_with_input(&host, &mut a, 10);
    let _source = a.set_clipboard(MIME, b"from the departed");
    a.roundtrip();

    focus_with_input(&host, &mut b, 20);
    pump_until_selections(&mut b, 1);
    assert!(b.has_clipboard_offer(), "B holds A's offer");
    let selections_before = b.selection_event_count();

    // A dies.
    drop(a);
    let h = scene.handle();
    assert!(
        h.wait_until(std::time::Duration::from_secs(5), |s| {
            s.get(SurfaceId(0)).is_none()
        }),
        "A's surface left the scene"
    );

    // B is still connected, still focused, and learns the clipboard is gone.
    for _ in 0..1000 {
        if b.selection_event_count() > selections_before {
            break;
        }
        b.roundtrip();
    }
    assert!(
        b.selection_event_count() > selections_before,
        "B was told the clipboard changed when its owner died"
    );
    assert!(
        !b.has_clipboard_offer(),
        "and the offer is gone, not left dangling at a dead client"
    );

    // B is otherwise fine: it can still copy.
    let _own = b.set_clipboard(MIME, b"B carries on");
    b.roundtrip();
    assert_eq!(b.source_cancelled_count(), 0, "B's own source is healthy");
}

/// Drag-and-drop is deferred, and **says so**: a client that starts a drag has
/// its source cancelled at once rather than waiting on a drag that will never
/// happen. Silence would be the dishonest option.
#[test]
fn starting_a_drag_cancels_the_source_rather_than_hanging() {
    let (_scene, host, mut clients) = fixture(1);
    let mut a = clients.pop().unwrap();

    let win = focus_with_input_returning_window(&host, &mut a, 10);

    // A drag is only legal in response to a pointer grab — a press on the
    // surface — so the test performs one, exactly as a dragging user would.
    host.input(InputEvent::PointerMotion {
        x: 5.0,
        y: 5.0,
        time_ms: 20,
    });
    host.input(InputEvent::PointerButton {
        button: parhelion_core::input::BTN_LEFT,
        pressed: true,
        time_ms: 21,
    });
    a.pump_until_input_events(5);

    let source = a.start_drag(&win);
    for _ in 0..1000 {
        if a.source_cancelled_count() > 0 {
            break;
        }
        a.roundtrip();
    }
    assert_eq!(
        a.source_cancelled_count(),
        1,
        "the drag source was cancelled — the client learns immediately that no \
         drag is happening"
    );
    drop(source);
}

/// As [`focus_with_input`], but hands back the window so the caller can start a
/// drag from its surface.
fn focus_with_input_returning_window(
    host: &ProtocolHost,
    client: &mut ScriptedClient,
    time_ms: u32,
) -> parhelion_harness::protocol_rig::Toplevel {
    let win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white());
    client.pump_until_input_events(1);
    host.input(InputEvent::Key {
        code: 30,
        pressed: true,
        time_ms,
    });
    host.input(InputEvent::Key {
        code: 30,
        pressed: false,
        time_ms: time_ms + 1,
    });
    client.pump_until_input_events(3);
    win
}

// ==========================================================================
// The end-to-end smoke, automated: two real third-party programs.
// ==========================================================================

/// `wl-copy` and `wl-paste` — programs from the `wl-clipboard` project, which
/// know nothing about Parhelion — exchange bytes through it.
///
/// This is the clipboard equivalent of the foot acceptance test: the rig tests
/// above prove the semantics with a client we wrote, and this proves the whole
/// thing works for software we did not.
///
/// **What it also demonstrates about the focus gate.** `wl-copy` cannot set the
/// selection without keyboard focus, and it knows it: traced with
/// `WAYLAND_DEBUG=1`, it creates an `xdg_toplevel`, waits to be focused, sets the
/// selection, and destroys the window again. The gate is not theoretical — real
/// clipboard tooling is built around it.
///
/// Skips loudly if `wl-clipboard` is not installed.
#[test]
fn wl_clipboard_tools_round_trip_real_bytes_through_the_compositor() {
    if !tool_available("wl-copy") || !tool_available("wl-paste") {
        eprintln!(
            "SKIPPING the wl-clipboard round-trip: wl-copy/wl-paste not installed. \
             (The rig tests above cover the same semantics with the scripted client.)"
        );
        return;
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("wayland-clipboard");

    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    host.listen_at(&socket).expect("bind the clipboard socket");
    host.set_output_size(200, 200);
    let presenter = host.frame_presenter();
    let h = scene.handle();
    let mut comp = CpuCompositor::new(200, 200, [0, 0, 0, 255]);
    let mut clock = 0u32;

    // wl-copy holds the data as long as it runs, so it stays in the foreground.
    let mut copy = Command::new("wl-copy")
        .env("WAYLAND_DISPLAY", &socket)
        .arg("--foreground")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wl-copy");
    copy.stdin
        .take()
        .expect("wl-copy stdin")
        .write_all(PAYLOAD)
        .expect("hand wl-copy the payload");

    // Tick while wl-copy does its work — without frames the compositor never gets
    // round to any of it — and wait on the compositor's own count of accepted
    // selections.
    //
    // Not on wl-copy's window: it maps one only to obtain keyboard focus (the
    // gate), sets the selection, and destroys it again within a millisecond, so
    // polling for a live surface samples right past it. That was a real flake in
    // an earlier version of this test, and the counter is the definite condition
    // it should have waited on.
    let deadline = Instant::now() + Duration::from_secs(30);
    while host.selections_set() == 0 {
        clock += 16;
        let snap = h.snapshot();
        comp.composite(&snap);
        presenter.present(clock);
        if Instant::now() >= deadline {
            // Kill first, *then* read: wl-copy --foreground keeps running, so its
            // stderr never reaches EOF and reading would hang the test instead of
            // failing it.
            let _ = copy.kill();
            let _ = copy.wait();
            let mut err = String::new();
            if let Some(mut e) = copy.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            panic!("wl-copy never set the selection; its stderr was: {err:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // Now paste, keeping the compositor ticking while wl-paste talks to it.
    let mut paste = Command::new("wl-paste")
        .env("WAYLAND_DISPLAY", &socket)
        .arg("--no-newline")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn wl-paste");
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        clock += 16;
        let snap = h.snapshot();
        comp.composite(&snap);
        presenter.present(clock);
        if let Some(status) = paste.try_wait().expect("poll wl-paste") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = paste.kill();
            let _ = copy.kill();
            panic!("wl-paste did not finish — the clipboard transfer stalled");
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let mut pasted = Vec::new();
    paste
        .stdout
        .take()
        .expect("wl-paste stdout")
        .read_to_end(&mut pasted)
        .expect("read wl-paste output");
    let _ = copy.kill();
    let _ = copy.wait();

    assert!(status.success(), "wl-paste exited cleanly");
    assert_eq!(
        pasted, PAYLOAD,
        "the bytes wl-copy published came back out of wl-paste, through Parhelion"
    );
    eprintln!(
        "clipboard round-trip: {} bytes crossed between two third-party programs",
        pasted.len()
    );
}

/// The payload for the round-trip. Deliberately not ASCII-only: a byte-for-byte
/// claim should be tested with bytes that a careless encoding step would mangle.
const PAYLOAD: &[u8] = "parhelion clipboard — ünïcödé ✓".as_bytes();

/// Whether an external tool is on `PATH`.
fn tool_available(name: &str) -> bool {
    Command::new(name)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
