//! **The M1 acceptance test**: a real terminal, that nobody wrote for us, runs
//! on Parhelion and echoes typed input — and typing redraws a *region*, not a
//! frame.
//!
//! Governing design: `docs/parhelion_milestone_plan.md` M1 acceptance;
//! `docs/plans/m1_tasks.md` T7. Everything M1 built exists for this test, and it
//! is deliberately **automated and headless** so the milestone's claim is a thing
//! CI re-proves on every push rather than an anecdote from one afternoon.
//!
//! # What is being proven
//!
//! 1. `foot` — a real, third-party, shm-rendering terminal — connects over a real
//!    Wayland socket, binds our globals, and reaches a **mapped** xdg toplevel.
//! 2. It commits real pixels (`bytes_copied` moves).
//! 3. Typing reaches it through the input funnel and **it redraws** — which is
//!    simultaneously the proof that **frame callbacks flow**, because foot
//!    throttles on them and will not paint again until the previous frame's
//!    callback arrives. Break the reverse path (T2) and this step fails; that is
//!    verified by sabotage, not assumed.
//! 4. The redraw is a small fraction of the output, with the pixels outside the
//!    damage region byte-identical. **This is VISION's founding thesis, measured.**
//! 5. Killing it unmaps cleanly and leaks no scene state.
//!
//! # Why counters and pixels, not goldens
//!
//! A terminal's output depends on its font stack, its config, and the machine's
//! fontconfig — a golden would pin someone's DejaVu version, not our compositor.
//! So the assertions are about *how much changed and where*, which is what M1
//! actually claims.
//!
//! # Determinism
//!
//! A real subprocess is not deterministic, so every wait is on a **definite
//! condition** (a scene predicate, a counter moving) with a generous budget and a
//! loud failure. Nothing sleeps waiting for something that has already happened.
//! If `foot` is not installed the test **skips with a loud message** rather than
//! failing — CI installs it.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_backend_headless::Frame;
use parhelion_core::input::InputEvent;
use parhelion_core::protocol::{FramePresenter, ProtocolHost};
use parhelion_core::render::Compositor;
use parhelion_core::scene::{Rect, SceneHandle, SceneThread, SnapshotDamage};

/// Output size for the acceptance run. Big enough for a terminal to lay out a
/// sensible grid, small enough to composite quickly on CI.
const W: u32 = 800;
const H: u32 = 600;
const CLEAR: [u8; 4] = [0, 0, 0, 255];

/// How long any single condition may take before the test fails loudly. Generous:
/// this waits on a real terminal starting, loading fonts, and spawning a shell.
const BUDGET: Duration = Duration::from_secs(30);

/// Milliseconds of simulated time per tick — the compositor's frame clock in this
/// test. 16 ms is ~60 Hz, which is what a client expects to be paced at.
const TICK_MS: u32 = 16;

/// **The proportionality bound.** A frame in which the terminal responds to a
/// keystroke may damage at most this fraction of the output.
///
/// Reasoning for the number: an 80×24 terminal cell is ~0.05% of the output, and
/// a full redrawn *line* is ~4%. Real terminals also repaint the cursor and
/// sometimes a scrollback row, so the honest bound is "much less than the frame",
/// not "one cell". 25% leaves an order of magnitude of headroom over any sane
/// terminal behaviour while still failing loudly if either side — foot's damage
/// reporting or our damage tracking — degenerated into repainting everything.
/// This is a **correctness** claim (damage is honoured), not a speed one; M5 owns
/// performance bounds.
const TYPING_DAMAGE_FRACTION_MAX: f64 = 0.25;

/// How many times the test will retype before giving up, and how long it waits
/// between attempts. Retrying defends against one race only — a terminal that has
/// mapped but whose shell has not yet attached to the pty — so it is deliberately
/// few and slow: a compositor that does not deliver keys must fail, not be
/// hammered until it looks like it works.
const MAX_TYPING_ROUNDS: u32 = 3;
const RETYPE_INTERVAL: Duration = Duration::from_secs(3);

/// How many consecutive damage-free frames count as "the terminal has settled".
/// Six at ~60 Hz is a tenth of a second of stillness — enough to be sure the
/// startup repaint is over, short enough not to pad the test.
const QUIET_TICKS: u32 = 6;

/// The sentence typed into the terminal, as evdev keycodes: "hello" + Enter.
/// Letters only — a shell will echo them, and no keymap-dependent punctuation is
/// involved, so this asserts the same thing on any machine.
const TYPED_KEYS: &[u32] = &[35, 18, 38, 38, 24, 28]; // h e l l o ⏎

/// A composited frame plus the damage that produced it.
struct Tick {
    damage: SnapshotDamage,
    frame: Frame,
}

/// One frame of the acceptance run, driven by hand so the test can see the
/// damage region (which `RenderLoop::tick` consumes internally).
///
/// This is exactly what `RenderLoop::tick` does — snapshot, composite, notify the
/// presenter so frame callbacks fire — unrolled for observability.
fn tick(
    scene: &SceneHandle,
    comp: &mut CpuCompositor,
    presenter: &FramePresenter,
    time_ms: u32,
) -> Tick {
    let snapshot = scene.snapshot();
    comp.composite(&snapshot);
    presenter.present(time_ms);
    Tick {
        damage: snapshot.damage,
        frame: comp.frame().clone(),
    }
}

/// Whether `foot` is available. Absence is a skip, not a failure: a developer
/// without it still gets the rest of the suite, and CI installs it.
fn foot_available() -> bool {
    Command::new("foot")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawn `foot` against `socket`, with a shell that does nothing but read input,
/// so what the test types is the only thing that happens.
fn spawn_foot(socket: &Path) -> Child {
    Command::new("foot")
        .env("WAYLAND_DISPLAY", socket) // absolute path: libwayland accepts it
        .args(["--title", "parhelion-acceptance", "--log-level=warning"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn foot")
}

/// Kill a child we spawned, by its exact PID, and reap it.
fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Total area of a damage region, in pixels (or the whole output for `Full`).
fn damage_area(damage: &SnapshotDamage) -> usize {
    match damage {
        SnapshotDamage::Full => (W * H) as usize,
        SnapshotDamage::Region(region) => region.rects().iter().map(|r| r.area()).sum(),
    }
}

/// Whether a point lies inside any damage rect.
fn in_damage(damage: &SnapshotDamage, x: u32, y: u32) -> bool {
    match damage {
        SnapshotDamage::Full => true,
        SnapshotDamage::Region(region) => region.rects().iter().any(|r| {
            let (x, y) = (x as i32, y as i32);
            x >= r.x && y >= r.y && x < r.right() && y < r.bottom()
        }),
    }
}

/// The whole arc, in one test, because the arc is the claim.
#[test]
fn a_real_terminal_runs_and_typing_redraws_a_region_not_a_frame() {
    if !foot_available() {
        eprintln!(
            "SKIPPING the M1 acceptance test: `foot` is not installed.\n\
             This test is the milestone's acceptance criterion — install foot to run it \
             (CI does)."
        );
        return;
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("wayland-acceptance");

    let scene = SceneThread::spawn();
    let host = ProtocolHost::new(scene.handle());
    host.listen_at(&socket).expect("bind the acceptance socket");
    host.set_output_size(W, H);
    let presenter = host.frame_presenter();
    let h = scene.handle();
    let mut comp = CpuCompositor::new(W, H, CLEAR);

    let mut foot = spawn_foot(&socket);
    let mut clock: u32 = 0;

    // ---- 1. It maps ------------------------------------------------------
    // Keep producing frames while waiting: a client that never gets a frame
    // callback stops drawing, so the compositor must be *running*, not merely
    // listening, for the terminal to reach a mapped state at all.
    let deadline = Instant::now() + BUDGET;
    loop {
        clock += TICK_MS;
        tick(&h, &mut comp, &presenter, clock);
        let mapped = h.query(|s| {
            (0..8)
                .map(parhelion_core::scene::SurfaceId)
                .filter_map(|id| s.get(id))
                .any(|n| n.is_visible() && n.role.toplevel().is_some())
        });
        if mapped {
            break;
        }
        if Instant::now() > deadline {
            kill_child(&mut foot);
            panic!("foot never reached a mapped toplevel within {BUDGET:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // The window is really foot's, and it really committed pixels.
    let app_id = h.query(|s| {
        (0..8)
            .map(parhelion_core::scene::SurfaceId)
            .filter_map(|id| s.get(id))
            .find_map(|n| n.role.toplevel().and_then(|t| t.app_id.clone()))
    });
    assert_eq!(
        app_id.as_deref(),
        Some("foot"),
        "the mapped toplevel identifies itself as foot"
    );
    assert!(
        host.bytes_copied() > 0,
        "the terminal committed real shm pixels"
    );

    // ---- 1b. Its decorations composite (M2 T7) ---------------------------
    // foot draws its title bar, borders and corners into subsurfaces. Until
    // subsurfaces landed we dropped them on the floor and rendered an undecorated
    // terminal — silently, which is what made it a debt rather than a bug report.
    // The scene tree is the evidence they are back.
    //
    // Waited for, not sampled: foot maps its root toplevel first and its
    // decorations a beat later, so checking the instant the window appears is a
    // race — one this test lost once under load before the loop was added.
    let count_subsurfaces = |h: &SceneHandle| {
        h.query(|s| {
            (0..32)
                .map(parhelion_core::scene::SurfaceId)
                .filter(|id| s.get(*id).is_some_and(|n| n.role.subsurface().is_some()))
                .filter(|id| s.is_mapped(*id))
                .count()
        })
    };
    let deadline = Instant::now() + BUDGET;
    let mut subsurfaces = count_subsurfaces(&h);
    while subsurfaces == 0 {
        clock += TICK_MS;
        tick(&h, &mut comp, &presenter, clock);
        subsurfaces = count_subsurfaces(&h);
        if Instant::now() > deadline {
            kill_child(&mut foot);
            panic!(
                "foot's decorations never composited: no mapped subsurface in the \
                 scene. Subsurface support is M2 T7; if this fails, it regressed."
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // And they are really on screen: the decoration pixels sit outside the
    // terminal's own content area, so the frame must differ from a frame drawn
    // from the root surface alone. Counters and pixels, not goldens — fonts and
    // themes are the client's business, not ours.
    let with_decorations = h.snapshot();
    assert!(
        with_decorations.len() > 1,
        "the snapshot carries the window *and* its decorations ({} nodes)",
        with_decorations.len()
    );

    // ---- 2. Settle ------------------------------------------------------
    // Wait for the terminal to go quiet — shell started, prompt drawn, nothing
    // moving. Two reasons this matters: the "before" frame must be a still one
    // for the comparison below to attribute the change to *typing*, and the
    // shell must have attached to the pty before we type at it, or the
    // keystrokes land in a void.
    let mut quiet = 0;
    let deadline = Instant::now() + BUDGET;
    while quiet < QUIET_TICKS && Instant::now() < deadline {
        clock += TICK_MS;
        let t = tick(&h, &mut comp, &presenter, clock);
        quiet = if damage_area(&t.damage) == 0 { quiet + 1 } else { 0 };
        std::thread::sleep(Duration::from_millis(5));
    }
    let settled = comp.frame().clone();
    let bytes_before_typing = host.bytes_copied();

    // ---- 3. Typing, and the frame-callback tripwire ---------------------
    // These are one step on purpose. foot throttles its drawing on frame
    // callbacks: it will not paint a new frame until the previous one's callback
    // arrives. So "it redrew in response to typing" *is* the proof that the
    // reverse path (T2) is alive — and if callbacks stop, this is where the test
    // fails, which is exactly what the sabotage check demonstrates.
    //
    // An earlier version waited for an unprompted second commit instead. That
    // passed locally and failed in CI, because an idle terminal with its prompt
    // already drawn has no reason to commit anything at all. Making the test
    // *cause* the redraw it waits for removed the flake and strengthened the
    // claim.
    let mut typing_frame: Option<Tick> = None;
    let mut previous_frame = settled.clone();
    let deadline = Instant::now() + BUDGET;
    let mut rounds = 0;
    let mut last_typed = Instant::now() - RETYPE_INTERVAL; // type immediately
    while typing_frame.is_none() {
        // Type the sentence, and retype a few times if nothing happens. A
        // terminal that has mapped may still be milliseconds away from having a
        // shell on the other end of its pty, and a swallowed first keystroke is a
        // race rather than a failure. Bounded and spaced, so a genuine "keys
        // never arrive" still fails on the budget below instead of being drowned
        // in retries.
        if rounds < MAX_TYPING_ROUNDS && last_typed.elapsed() >= RETYPE_INTERVAL {
            for (i, &code) in TYPED_KEYS.iter().enumerate() {
                let t = clock + (i as u32 + 1) * TICK_MS;
                host.input(InputEvent::Key {
                    code,
                    pressed: true,
                    time_ms: t,
                });
                host.input(InputEvent::Key {
                    code,
                    pressed: false,
                    time_ms: t + 1,
                });
            }
            rounds += 1;
            last_typed = Instant::now();
        }

        // `prev` is the frame *immediately* before this one — which is what the
        // outside-damage assertion must compare against. Comparing against the
        // settled frame instead was a real defect (and a flake): if the terminal
        // repaints across two ticks, the pixels changed by the first tick are
        // outside the second tick's damage, and the assertion fails for a
        // compositor that did nothing wrong.
        let prev = comp.frame().clone();
        clock += TICK_MS;
        let t = tick(&h, &mut comp, &presenter, clock);
        if damage_area(&t.damage) > 0 && t.frame.pixels() != prev.pixels() {
            previous_frame = prev;
            typing_frame = Some(t);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));

        if Instant::now() > deadline {
            kill_child(&mut foot);
            panic!(
                "the terminal never redrew after {rounds} round(s) of typing. Either the keys \
                 did not reach it, or frame callbacks are not flowing — foot throttles on them \
                 and will not paint again until the previous frame's callback arrives. \
                 (bytes_copied {} → {})",
                bytes_before_typing,
                host.bytes_copied()
            );
        }
    }
    let typed = typing_frame.expect("a typing frame");
    assert!(
        host.bytes_copied() > bytes_before_typing,
        "the terminal committed new pixels in response to typing"
    );

    // ---- 4. The founding thesis, measured -------------------------------
    let area = damage_area(&typed.damage);
    let fraction = area as f64 / (W * H) as f64;
    // Reported unconditionally: the number *is* the milestone's evidence, and a
    // CI log that only says "ok" cannot be read back later.
    eprintln!(
        "M1 acceptance: typing damaged {area} px of {} ({:.2}% of the output; bound {:.0}%)",
        W * H,
        fraction * 100.0,
        TYPING_DAMAGE_FRACTION_MAX * 100.0
    );
    assert!(
        fraction <= TYPING_DAMAGE_FRACTION_MAX,
        "typing redrew {:.1}% of the output ({area} px) — the bound is {:.0}%. \
         Typing must redraw a region, not a frame.",
        fraction * 100.0,
        TYPING_DAMAGE_FRACTION_MAX * 100.0
    );

    // Across this one frame: outside its damage region nothing moved, inside it
    // something did.
    let mut changed_inside = 0usize;
    let mut changed_outside = 0usize;
    for y in 0..H {
        for x in 0..W {
            if previous_frame.pixel(x, y) == typed.frame.pixel(x, y) {
                continue;
            }
            if in_damage(&typed.damage, x, y) {
                changed_inside += 1;
            } else {
                changed_outside += 1;
            }
        }
    }
    assert_eq!(
        changed_outside, 0,
        "pixels outside the damage region changed — damage under-reported the redraw"
    );
    assert!(
        changed_inside > 0,
        "no pixel inside the damage region changed — the terminal did not actually redraw"
    );
    eprintln!("M1 acceptance: {changed_inside} px changed, all inside the damage region");

    // And the terminal really did respond to the typing, rather than merely
    // finishing something it had already started: the screen differs from the
    // settled, pre-typing frame.
    assert_ne!(
        typed.frame.pixels(),
        settled.pixels(),
        "the screen changed after typing, not just within one repaint"
    );

    // ---- 5. Teardown ----------------------------------------------------
    kill_child(&mut foot);
    let emptied = h.wait_until(BUDGET, |s| s.surface_count() == 0);
    for _ in 0..10 {
        clock += TICK_MS;
        tick(&h, &mut comp, &presenter, clock);
    }
    assert!(
        emptied,
        "the terminal's surfaces left the scene when it died — no leaked nodes"
    );
    assert!(
        h.snapshot().is_empty(),
        "and nothing is left to composite"
    );
}

/// Keep `Rect` in the imports honest — the damage helpers above use its accessors
/// through `SnapshotDamage`, and this makes the dependency explicit to a reader.
#[allow(dead_code)]
fn _rect_shape(r: Rect) -> i32 {
    r.right() + r.bottom()
}
