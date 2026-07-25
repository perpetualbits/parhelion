//! T7 conformance sweep: the error paths of the globals Parhelion advertises.
//!
//! Governing design: `docs/plans/m1_tasks.md` T7 ("conformance sweep for all
//! implemented globals"). The other conformance tests live with the feature they
//! belong to — xdg-shell's three protocol errors in `xdg.rs`, the frame-callback
//! lifecycle in `protocol.rs`, the seat's delivery rules in `input.rs`, the
//! output's advertisement in `output.rs`. This file holds what the sweep found
//! *missing*: the `wl_shm` rejection path, and `xdg_output`'s logical geometry.
//!
//! The standard every one of them meets: assert the **specific** error, never
//! merely that the client was disconnected. A compositor that kills a client for
//! the wrong reason is still broken.

use std::os::unix::net::UnixStream;

use parhelion_core::protocol::{ProtocolHost, DEFAULT_OUTPUT_SIZE};
use parhelion_core::scene::SceneThread;
use parhelion_harness::protocol_rig::{ScriptedClient, ShmFormat};

fn fixture() -> (SceneThread, ProtocolHost, ScriptedClient) {
    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    let (server_end, client_end) = UnixStream::pair().expect("socketpair");
    host.add_client(server_end);
    let client = ScriptedClient::connect(client_end);
    (scene, host, client)
}

/// `wl_shm.error.invalid_stride` — the code the compositor posts when a buffer's
/// claimed geometry does not fit the pool backing it. (The errors are declared on
/// `wl_shm`; they are *posted* on the `wl_shm_pool` object that could not satisfy
/// the request.)
const WL_SHM_ERROR_INVALID_STRIDE: u32 = 1;

/// A buffer whose rows do not fit its pool is rejected — not accepted, and
/// *certainly* not read out of bounds.
///
/// This is the hostile-input boundary in miniature: the pool is client-owned
/// shared memory, the geometry is a client-supplied claim about it, and the
/// compositor's job is to disbelieve the claim. Parhelion's own copy path has a
/// second guard of its own (`build_pixel_block` re-checks stride and offset
/// against the mapping before reading a byte), so this is defence in depth — but
/// the protocol-level rejection is what a client is entitled to.
#[test]
fn a_buffer_that_does_not_fit_its_pool_is_rejected() {
    let (_scene, _host, mut client) = fixture();

    // A 16×16 pool, then a buffer claiming rows four times as wide as they are.
    let pool = client.create_pool(16 * 16 * 4);
    let _buffer = client.create_buffer_raw(&pool, 0, 16, 16, 16 * 16, ShmFormat::Xrgb8888);

    let err = client.expect_protocol_error();
    assert_eq!(
        err.interface, "wl_shm_pool",
        "the error is posted on the pool that could not satisfy the request"
    );
    assert_eq!(
        err.code, WL_SHM_ERROR_INVALID_STRIDE,
        "wl_shm.error.invalid_stride ({})",
        err.message
    );
}

/// A buffer offset past the end of its pool is rejected by the same guard — the
/// case an out-of-bounds read would otherwise fall straight through.
#[test]
fn a_buffer_offset_past_the_pool_is_rejected() {
    let (_scene, _host, mut client) = fixture();

    let pool = client.create_pool(16 * 16 * 4);
    // Offset a whole pool's worth past the start.
    let _buffer = client.create_buffer_raw(&pool, 16 * 16 * 4, 4, 4, 16, ShmFormat::Xrgb8888);

    let err = client.expect_protocol_error();
    assert_eq!(err.interface, "wl_shm_pool");
    assert_eq!(
        err.code, WL_SHM_ERROR_INVALID_STRIDE,
        "out-of-range geometry is rejected with the same code ({})",
        err.message
    );
}

/// The registry advertises exactly the set of globals Parhelion means to serve.
///
/// **On `wl_subcompositor`:** it is advertised *ahead of its support*, which the
/// governing decision permits on one condition — every unsupported request on it
/// must refuse loudly (see `asking_for_a_subsurface_is_refused_out_loud`).
/// Withdrawing it instead was tried and measured: `foot` then refuses to start
/// (`no sub compositor`, exit 230), because clients hard-gate on the presence of
/// globals they never use. The assertion below pins *what is advertised*, so any
/// future change to that set is deliberate rather than a side effect.
#[test]
fn the_registry_advertises_exactly_the_expected_globals() {
    let (_scene, _host, client) = fixture();
    let globals: Vec<String> = client
        .advertised_globals()
        .into_iter()
        .map(|(interface, _)| interface)
        .collect();

    for expected in [
        "wl_compositor",
        "wl_shm",
        "wl_seat",
        "wl_output",
        "xdg_wm_base",
        "wl_data_device_manager",
        "zxdg_output_manager_v1",
    ] {
        assert!(
            globals.iter().any(|g| g == expected),
            "{expected} is advertised (globals: {globals:?})"
        );
    }

    // Pinned deliberately, debt and all: changing what we advertise should be a
    // decision, not a side effect of a dependency bump.
    assert!(
        globals.iter().any(|g| g == "wl_subcompositor"),
        "wl_subcompositor is advertised (a stated debt — see this test's docs)"
    );
    assert_eq!(
        globals.len(),
        8,
        "no global is advertised that this test does not name: {globals:?}"
    );
}

/// **Subsurfaces: what a client can do, and what silently does not happen.**
///
/// This test pins the *current, unhappy* truth rather than a wish. Parhelion
/// advertises `wl_subcompositor` and lets clients build subsurfaces — and then
/// does not composite them, because the scene draws only root surfaces until
/// M2 T7.
///
/// M2 T0 tried to make that refusal loud at both candidate points (creating a
/// subsurface; committing content to one) and **both kill `foot`**: it creates
/// nine subsurfaces at startup and puts real pixels in eight of them (its
/// client-side decorations). Loud refusal and a working terminal are mutually
/// exclusive until subsurfaces exist. The evidence is in the T0 session summary;
/// the decision log carries the correction that overturned T0's premise.
///
/// **This test inverts at M2 T7**, when the content below actually appears.
#[test]
fn a_subsurface_is_accepted_and_its_content_is_silently_not_composited() {
    let (scene, _host, mut client) = fixture();

    let parent = client.create_surface();
    let child = client.create_surface();
    let _subsurface = client.get_subsurface(&child, &parent);

    let mut pool = client.create_pool(8 * 8 * 4);
    pool.write(&[255u8; 8 * 8 * 4]);
    let buffer = client.create_buffer(&pool, 8, 8, ShmFormat::Xrgb8888);
    client.attach(&child, &buffer);
    client.commit(&child);
    client.commit(&parent); // sync subsurface: content lands with the parent
    client.roundtrip();

    assert!(
        client.protocol_error().is_none(),
        "the client is not refused — it cannot be, and foot is the proof"
    );
    assert!(
        scene.handle().snapshot().is_empty(),
        "and its content is not composited: nothing reached the scene. This is the \
         debt M2 T7 pays; if this assertion starts failing, T7 has landed and this \
         test should invert."
    );
}

/// `xdg_output` reports the output's **logical** geometry, and at scale 1 that is
/// its mode. Advertised alongside `wl_output`, so it is tested alongside it
/// rather than left as untested surface area.
#[test]
fn xdg_output_reports_logical_geometry() {
    let (_scene, _host, mut client) = fixture();
    let (w, h) = client.xdg_output_logical_size();
    assert_eq!(
        (w as u32, h as u32),
        DEFAULT_OUTPUT_SIZE,
        "logical size equals the mode at scale 1"
    );
    assert_eq!(
        client.xdg_output_logical_position(),
        (0, 0),
        "the single output sits at the origin"
    );
}
