//! T6 input tests: the seat, the funnel, and the C10 focus fallback.
//!
//! Governing design: `docs/scene_graph_v1.md` §11 (input), `CORE-BOUNDARY.md`
//! §3 (C2), §7 (T-input), and invariant I-2. These drive a real `ProtocolHost`
//! with scripted clients and inject events through `ProtocolHost::input` — the
//! same funnel the winit backend feeds, so what CI exercises is the production
//! path minus winit's translation (which has its own unit tests in
//! `parhelion-backend-winit`).
//!
//! **Ordering is half of what these assert.** A client must never see a key
//! before the `enter` that gave it focus, or an `enter` for a new surface before
//! the `leave` from the old one — so the rig records all seat events in one
//! ordered list and the tests assert on slices of it.
//!
//! Determinism: every event is injected by the test, timestamps are supplied by
//! the test, and waits are on definite conditions (an event already in flight),
//! never sleeps.

use std::os::unix::net::UnixStream;

use parhelion_core::input::{InputEvent, BTN_LEFT, BTN_RIGHT};
use parhelion_core::protocol::{ProtocolHost, CASCADE_STEP_X, CASCADE_STEP_Y};
use parhelion_core::scene::SceneThread;
use parhelion_harness::protocol_rig::{ScriptedClient, SeatEvent, ShmFormat};

/// Window size for these tests.
const W: i32 = 20;
const H: i32 = 20;

/// evdev keycodes used below (`linux/input-event-codes.h`).
const KEY_A: u32 = 30;
const KEY_LEFTSHIFT: u32 = 42;

/// Opaque white `wl_shm` bytes for a `w×h` window.
fn white(w: i32, h: i32) -> Vec<u8> {
    vec![255u8; (w * h * 4) as usize]
}

/// Just the keyboard focus changes from a client's event log. `wl_keyboard.enter`
/// is always accompanied by a `modifiers` event (the protocol requires the client
/// to learn the modifier state on focus), which is noise when the question is
/// "who has focus, and in what order did it move?".
fn focus_events(client: &ScriptedClient) -> Vec<SeatEvent> {
    client
        .input_events()
        .iter()
        .filter(|e| {
            matches!(
                e,
                SeatEvent::KeyboardEnter { .. } | SeatEvent::KeyboardLeave { .. }
            )
        })
        .cloned()
        .collect()
}

/// Scene + host + one connected client.
fn fixture() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

// ==========================================================================
// The seat itself.
// ==========================================================================

/// The seat advertises keyboard + pointer, and sends a keymap that is real xkb
/// text — not an empty fd, which is the failure mode a "keymap received" check
/// alone would sail past.
#[test]
fn seat_advertises_capabilities_and_sends_a_keymap() {
    let (_scene, _host, mut client) = fixture();
    client.roundtrip();
    client.roundtrip();

    // wl_seat.capability: pointer = 1, keyboard = 2. Both, no touch.
    let caps = client.seat_capabilities();
    assert_eq!(caps & 1, 1, "pointer capability advertised");
    assert_eq!(caps & 2, 2, "keyboard capability advertised");
    assert_eq!(caps & 4, 0, "no touch capability in M1");

    let keymap = client.keymap().expect("keymap delivered");
    assert!(!keymap.is_empty(), "keymap is not empty");
    assert!(
        keymap.contains("xkb_keymap"),
        "keymap parses as xkb text, starts: {:?}",
        &keymap[..keymap.len().min(40)]
    );
}

// ==========================================================================
// Keyboard.
// ==========================================================================

/// Keys reach the focused client with the evdev code the funnel was given,
/// serials increase, and a modifier key produces a `modifiers` event.
#[test]
fn keys_reach_the_focused_client_with_evdev_codes_and_modifiers() {
    let (_scene, host, mut client) = fixture();
    let _win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    client.pump_until_input_events(1); // the enter that mapping earned
    client.clear_input_events();

    // Shift down, A down, A up, shift up — the shape of typing a capital.
    host.input(InputEvent::Key {
        code: KEY_LEFTSHIFT,
        pressed: true,
        time_ms: 10,
    });
    host.input(InputEvent::Key {
        code: KEY_A,
        pressed: true,
        time_ms: 20,
    });
    host.input(InputEvent::Key {
        code: KEY_A,
        pressed: false,
        time_ms: 30,
    });
    // 3 keys (shift itself is a key) + the modifiers event shift raises.
    client.pump_until_input_events(4);

    let events = client.input_events().to_vec();
    let keys: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SeatEvent::Key {
                key,
                pressed,
                serial,
            } => Some((*key, *pressed, *serial)),
            _ => None,
        })
        .collect();
    assert_eq!(
        keys.iter().map(|(k, p, _)| (*k, *p)).collect::<Vec<_>>(),
        vec![(KEY_LEFTSHIFT, true), (KEY_A, true), (KEY_A, false)],
        "evdev codes and press/release states arrive unchanged, in order"
    );

    // Serials are strictly increasing across the whole seat, not just per device.
    let serials: Vec<u32> = keys.iter().map(|(_, _, s)| *s).collect();
    assert!(
        serials.windows(2).all(|w| w[0] < w[1]),
        "serials increase monotonically: {serials:?}"
    );

    // Shift produced a modifiers event with a non-zero depressed mask, before
    // the 'a' that it modifies.
    let mods_idx = events
        .iter()
        .position(|e| matches!(e, SeatEvent::Modifiers { depressed } if *depressed != 0))
        .expect("a modifiers event with shift held");
    let a_idx = events
        .iter()
        .position(|e| matches!(e, SeatEvent::Key { key, pressed: true, .. } if *key == KEY_A))
        .expect("the 'a' press");
    assert!(
        mods_idx < a_idx,
        "modifiers arrive before the key they modify: {events:?}"
    );
}

/// Keyboard focus follows the topmost mapped toplevel (the C10 fallback): map A,
/// map B (B takes focus, A is left), unmap B (A is re-focused). The enter/leave
/// sequence is asserted, not just the end state.
#[test]
fn focus_follows_the_topmost_toplevel_across_map_and_unmap() {
    let (_scene, _host, mut client) = fixture();

    let a = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let a_id = client.surface_id(&a.surface);
    client.pump_until_input_events(1);
    assert_eq!(
        focus_events(&client),
        vec![SeatEvent::KeyboardEnter { surface: a_id }],
        "the first window to map takes focus"
    );
    client.clear_input_events();

    // B maps on top (later cascade slot, higher SurfaceId → topmost).
    let b = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let b_id = client.surface_id(&b.surface);
    client.pump_until_input_events(2);
    assert_eq!(
        focus_events(&client),
        vec![
            SeatEvent::KeyboardLeave { surface: a_id },
            SeatEvent::KeyboardEnter { surface: b_id },
        ],
        "focus moves to the new topmost window: leave A, then enter B"
    );
    client.clear_input_events();

    // B unmaps → focus falls back to A, the only window left.
    client.attach_null(&b.surface);
    client.commit(&b.surface);
    client.roundtrip();
    client.pump_until_input_events(2);
    assert_eq!(
        focus_events(&client),
        vec![
            SeatEvent::KeyboardLeave { surface: b_id },
            SeatEvent::KeyboardEnter { surface: a_id },
        ],
        "unmapping the focused window re-focuses what is left"
    );
}

/// The T5 rule meets input: a surface that is not a mapped toplevel never
/// receives keyboard focus or key events — not a roleless `wl_surface`, and not
/// a toplevel that has unmapped.
#[test]
fn unmapped_and_roleless_surfaces_never_receive_input() {
    let (_scene, host, mut client) = fixture();

    // A bare wl_surface with a committed buffer: displayed nowhere, focused never.
    let surface = client.create_surface();
    let mut pool = client.create_pool((W * H * 4) as usize);
    pool.write(&white(W, H));
    let buffer = client.create_buffer(&pool, W, H, ShmFormat::Xrgb8888);
    client.attach(&surface, &buffer);
    client.commit(&surface);
    client.roundtrip();

    host.input(InputEvent::Key {
        code: KEY_A,
        pressed: true,
        time_ms: 10,
    });
    // The pointer is over where the surface would be if it were displayed.
    host.input(InputEvent::PointerMotion {
        x: 5.0,
        y: 5.0,
        time_ms: 20,
    });
    host.input(InputEvent::PointerButton {
        button: BTN_LEFT,
        pressed: true,
        time_ms: 30,
    });
    client.roundtrip();
    client.roundtrip();
    assert_eq!(
        client.input_events(),
        &[],
        "a roleless surface receives no focus, no keys, no pointer"
    );

    // Now map a real toplevel, then unmap it: input stops again.
    let win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    client.pump_until_input_events(1);
    client.attach_null(&win.surface);
    client.commit(&win.surface);
    client.roundtrip();
    client.clear_input_events();

    host.input(InputEvent::Key {
        code: KEY_A,
        pressed: true,
        time_ms: 40,
    });
    host.input(InputEvent::PointerMotion {
        x: 5.0,
        y: 5.0,
        time_ms: 50,
    });
    client.roundtrip();
    client.roundtrip();
    assert_eq!(
        client.input_events(),
        &[],
        "an unmapped toplevel receives nothing further"
    );
}

// ==========================================================================
// Pointer.
// ==========================================================================

/// Moving the cursor across two cascaded windows produces a leave/enter pair
/// with **surface-local** coordinates — the number that would be wrong if the
/// hit test forgot the window's placement.
#[test]
fn pointer_crossing_two_windows_reports_surface_local_coordinates() {
    let (_scene, host, mut client) = fixture();

    // A at the cascade origin, B one step down-right; both 20×20, so B covers
    // (32,32)..(52,52) and A covers (0,0)..(20,20) — they do not overlap.
    let a = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let b = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let (a_id, b_id) = (client.surface_id(&a.surface), client.surface_id(&b.surface));
    client.pump_until_input_events(1);
    client.clear_input_events();

    // Into A, at (5,5) output = (5,5) local.
    host.input(InputEvent::PointerMotion {
        x: 5.0,
        y: 5.0,
        time_ms: 10,
    });
    client.pump_until_input_events(1);
    assert_eq!(
        client.input_events()[0],
        SeatEvent::PointerEnter {
            surface: a_id,
            x: 5.0,
            y: 5.0
        },
        "entering A reports coordinates local to A"
    );
    client.clear_input_events();

    // Into B, at output (CASCADE_STEP + 3) = local (3,3).
    let (bx, by) = (CASCADE_STEP_X as f64 + 3.0, CASCADE_STEP_Y as f64 + 3.0);
    host.input(InputEvent::PointerMotion {
        x: bx,
        y: by,
        time_ms: 20,
    });
    client.pump_until_input_events(2);
    assert_eq!(
        client.input_events(),
        &[
            SeatEvent::PointerLeave { surface: a_id },
            SeatEvent::PointerEnter {
                surface: b_id,
                x: 3.0,
                y: 3.0
            },
        ],
        "crossing leaves A before entering B, with B-local coordinates"
    );
    client.clear_input_events();

    // Off both windows: leave, and nothing entered.
    host.input(InputEvent::PointerMotion {
        x: 200.0,
        y: 200.0,
        time_ms: 30,
    });
    client.pump_until_input_events(1);
    assert_eq!(
        client.input_events(),
        &[SeatEvent::PointerLeave { surface: b_id }],
        "leaving the last window enters nothing"
    );
}

/// A click goes to the window under the cursor, and only after that window has
/// been entered — the enter-before-input ordering, from the pointer side.
#[test]
fn button_goes_to_the_window_under_the_cursor_after_its_enter() {
    let (_scene, host, mut client) = fixture();
    let a = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let b = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    let (a_id, b_id) = (client.surface_id(&a.surface), client.surface_id(&b.surface));
    client.pump_until_input_events(1);
    client.clear_input_events();

    // Move into B and click.
    host.input(InputEvent::PointerMotion {
        x: CASCADE_STEP_X as f64 + 5.0,
        y: CASCADE_STEP_Y as f64 + 5.0,
        time_ms: 10,
    });
    host.input(InputEvent::PointerButton {
        button: BTN_LEFT,
        pressed: true,
        time_ms: 20,
    });
    host.input(InputEvent::PointerButton {
        button: BTN_LEFT,
        pressed: false,
        time_ms: 30,
    });
    client.pump_until_input_events(3);

    let events = client.input_events().to_vec();
    let enter_idx = events
        .iter()
        .position(|e| matches!(e, SeatEvent::PointerEnter { surface, .. } if *surface == b_id))
        .expect("entered B");
    let button_idx = events
        .iter()
        .position(|e| matches!(e, SeatEvent::PointerButton { pressed: true, .. }))
        .expect("button press delivered");
    assert!(
        enter_idx < button_idx,
        "no button before the enter that focuses its surface: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SeatEvent::PointerEnter { surface, .. } if *surface == a_id)),
        "A was never entered — the click belongs to B alone: {events:?}"
    );

    let buttons: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SeatEvent::PointerButton {
                button, pressed, ..
            } => Some((*button, *pressed)),
            _ => None,
        })
        .collect();
    assert_eq!(
        buttons,
        vec![(BTN_LEFT, true), (BTN_LEFT, false)],
        "press and release both delivered, with the BTN_ code intact"
    );

    // A different button code arrives unchanged too (the funnel is not
    // special-casing left).
    client.clear_input_events();
    host.input(InputEvent::PointerButton {
        button: BTN_RIGHT,
        pressed: true,
        time_ms: 40,
    });
    client.pump_until_input_events(1);
    assert_eq!(
        client.input_events(),
        &[SeatEvent::PointerButton {
            button: BTN_RIGHT,
            pressed: true,
            serial: match client.input_events()[0] {
                SeatEvent::PointerButton { serial, .. } => serial,
                _ => unreachable!(),
            }
        }]
    );
}

/// Scrolling reaches the focused window — cheap to support now, painful to
/// retrofit later, and terminals scroll.
#[test]
fn axis_events_reach_the_window_under_the_cursor() {
    let (_scene, host, mut client) = fixture();
    let _win = client.map_toplevel(W, H, ShmFormat::Xrgb8888, &white(W, H));
    client.pump_until_input_events(1);

    // Put the cursor in the window first and wait for its `enter` specifically —
    // clearing before that arrives would leave it to show up mid-assertion.
    client.clear_input_events();
    host.input(InputEvent::PointerMotion {
        x: 5.0,
        y: 5.0,
        time_ms: 10,
    });
    client.pump_until_input_events(1);
    assert!(
        matches!(client.input_events()[0], SeatEvent::PointerEnter { .. }),
        "the cursor is in the window before it scrolls"
    );
    client.clear_input_events();

    host.input(InputEvent::PointerAxis {
        horizontal: 0.0,
        vertical: 10.0,
        steps: 1,
        time_ms: 20,
    });
    client.pump_until_input_events(1);

    // wl_pointer.axis: 0 = vertical scroll, 1 = horizontal.
    assert_eq!(
        client.input_events(),
        &[SeatEvent::PointerAxis {
            axis: 0,
            value: 10.0
        }],
        "a vertical scroll arrives with its value intact"
    );
}
