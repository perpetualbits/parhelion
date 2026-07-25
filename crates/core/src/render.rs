//! The render side of the core: the compositor seam, the frame counters, and
//! the T-render skeleton (`docs/CORE-BOUNDARY.md` §3 C5, §7 T-render).
//!
//! Canonical doc: `docs/scene_graph_v1.md`. This module owns the *how* of
//! turning an immutable [`Snapshot`] into a frame — the C5 "record + submit"
//! half — while staying ignorant of *what a frame is*. That ignorance is
//! deliberate: the concrete pixel target (an in-memory `Frame` for the headless
//! backend now; a GPU/DRM target in M2) lives in a backend crate, behind the
//! [`Compositor`] seam. Keeping `Frame` out of the core means the core names no
//! specific backend — the same discipline that keeps Smithay's renderer types
//! out of here (decision "2026-07-24 — Smithay threading fit").
//!
//! # Frame-path obligations
//!
//! Code reached from [`RenderLoop::tick`] runs on T-render (§7). It must honour
//! I-1 (never block: no locks shared with the scene thread — the snapshot is an
//! owned value, not a borrow; no synchronous IPC) and I-3 (no synchronous call
//! into any server). In M1 the tick is driven by the test, not a vblank clock;
//! the real frame scheduler (render-as-late-as-possible) arrives with the DRM
//! backend in M2.

use crate::protocol::FramePresenter;
use crate::scene::{SceneHandle, Snapshot, SnapshotDamage};

/// The seam between the core's render loop and a concrete pixel target.
///
/// An implementor owns some frame buffer (in-memory, GPU, …) and knows how to
/// paint a [`Snapshot`] into it. The core drives it through this one method and
/// never learns the buffer's type — that is what lets `crates/core` depend on no
/// backend. The headless backend's `CpuCompositor` is the M1 implementor; M2's
/// DRM backend implements the same trait.
///
/// # Contract
///
/// - Composites the snapshot's nodes **back-to-front** (the snapshot is already
///   in that order; the implementor iterates as-is).
/// - Returns the number of nodes actually composited, for the frame counter
///   ([`FrameCounters::nodes_composited`]).
/// - Runs on the frame path (I-1): the implementation must not block, allocate
///   unboundedly, or call into a server synchronously (I-3).
pub trait Compositor {
    /// Composite `snapshot` into this compositor's target. The implementor keeps
    /// its previous frame and recomputes only within the snapshot's damage
    /// ([`Snapshot::damage`]) — pixels outside damage keep their retained values.
    /// Returns per-frame [`CompositeStats`] for the counters.
    fn composite(&mut self, snapshot: &Snapshot) -> CompositeStats;
}

/// What one [`Compositor::composite`] did, folded into [`FrameCounters`] by the
/// render loop. Kept minimal on purpose.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompositeStats {
    /// Nodes touched this frame (drawn at least partly).
    pub nodes_composited: usize,
    /// Pixels redrawn this frame — the sum of damage-rect areas actually painted
    /// (an upper bound when damage rects overlap; damage never subtracts).
    pub pixels_redrawn: usize,
}

/// Per-frame instrumentation — the *mechanism* T4 (damage tracking) extends with
/// pixels-redrawn / region counts. Kept minimal on purpose (`CLAUDE.md`: no
/// abstraction beyond what the task requires); today it answers only "how many
/// frames did the render loop produce, and how many nodes did they composite?"
///
/// These are monotonic counters over the render loop's lifetime, not per-frame
/// deltas — a test reads them after N ticks and checks totals.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrameCounters {
    /// Total frames the render loop has produced (one per [`RenderLoop::tick`]).
    pub frames_produced: u64,
    /// Total nodes composited across all frames produced.
    pub nodes_composited: u64,
    /// Total pixels redrawn across all frames — the damage-tracking savings are
    /// visible as this growing far slower than `frames_produced * output_area`.
    pub pixels_redrawn: u64,
    /// Total damage rects processed across all `Region`-damage frames.
    pub damage_rects: u64,
    /// Frames that fell back to full-output damage (first frame + "don't know"
    /// cases). Counted so the conservative fallback's frequency stays visible.
    pub full_damage_frames: u64,
}

/// The T-render skeleton: pull a [`Snapshot`] from the scene, hand it to the
/// [`Compositor`], and count the frame. Generic over the compositor so the core
/// carries no backend type (`C` is the headless `CpuCompositor` in M1, a DRM
/// target in M2).
///
/// # The tick, and what M1 leaves out
///
/// [`tick`](Self::tick) is one frame's worth of work, driven **externally** — a
/// test calls it, so frame production is deterministic and has no wall-clock
/// (`docs/harness_design.md` §4). The real thing this skeletons — the
/// render-as-late-as-possible frame scheduler tied to vblank (§7 T-commit) —
/// arrives with the DRM backend (M2); until then "when to tick" is the caller's.
/// There is no damage yet (T4): every tick composites the whole snapshot.
///
/// Ownership: this is T-render's state. It shares no lock with the scene thread
/// — it pulls an owned [`Snapshot`] over [`SceneHandle::snapshot`] (I-1).
pub struct RenderLoop<C: Compositor> {
    /// Handle to the scene owner; the source of per-frame snapshots.
    scene: SceneHandle,
    /// The pixel target this loop paints into.
    compositor: C,
    /// Running frame/nodes instrumentation.
    counters: FrameCounters,
    /// Optional reverse edge to the protocol side (T2). When present, each tick
    /// notifies it that a frame was presented, so `wl_surface.frame` callbacks
    /// fire. `None` for render-only paths (e.g. golden tests with no protocol
    /// host); the notice is the *only* thing T-render tells T-proto, and it is
    /// non-blocking (I-1) — see [`FramePresenter`].
    presenter: Option<FramePresenter>,
}

impl<C: Compositor> RenderLoop<C> {
    /// Build a render loop over `scene` (snapshot source) and `compositor`
    /// (pixel target). Produces no frame until [`tick`](Self::tick) is called,
    /// and fires no callbacks until a [`FramePresenter`] is attached with
    /// [`with_presenter`](Self::with_presenter).
    pub fn new(scene: SceneHandle, compositor: C) -> Self {
        RenderLoop {
            scene,
            compositor,
            counters: FrameCounters::default(),
            presenter: None,
        }
    }

    /// Attach the reverse edge (T2): after this, each [`tick`](Self::tick) tells
    /// the protocol side a frame was presented, firing that frame's
    /// `wl_surface.frame` callbacks. Obtain the presenter from
    /// [`ProtocolHost::frame_presenter`](crate::protocol::ProtocolHost::frame_presenter).
    pub fn with_presenter(mut self, presenter: FramePresenter) -> Self {
        self.presenter = Some(presenter);
        self
    }

    /// Produce one frame: pull the current [`Snapshot`] (scene→render edge),
    /// composite it, advance the counters, and — if a presenter is attached —
    /// notify the protocol side that a frame was presented at `time_ms` (the
    /// reverse edge, firing frame callbacks). Test-controlled — `time_ms` is the
    /// deterministic timestamp the callbacks report (monotonic ms per protocol);
    /// there is no timer.
    pub fn tick(&mut self, time_ms: u32) {
        let snapshot = self.scene.snapshot();
        let stats = self.compositor.composite(&snapshot);
        self.counters.frames_produced += 1;
        self.counters.nodes_composited += stats.nodes_composited as u64;
        self.counters.pixels_redrawn += stats.pixels_redrawn as u64;
        // Damage accounting: a full-damage frame is the conservative fallback;
        // otherwise count the rects the region carried.
        match &snapshot.damage {
            SnapshotDamage::Full => self.counters.full_damage_frames += 1,
            SnapshotDamage::Region(region) => {
                self.counters.damage_rects += region.len() as u64;
            }
        }

        // Reverse edge (T-render → T-proto). Non-blocking enqueue only (I-1); the
        // dispatch thread owns the actual `wl_callback.done` sends and the flush.
        if let Some(presenter) = &self.presenter {
            presenter.present(time_ms);
        }
    }

    /// The instrumentation counters (frames produced, nodes composited so far).
    pub fn counters(&self) -> &FrameCounters {
        &self.counters
    }

    /// The compositor, for reading back the produced frame (e.g. a golden test
    /// fetching pixels — the core itself never interprets them).
    pub fn compositor(&self) -> &C {
        &self.compositor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{ClientKey, SceneThread, SurfaceId, Transform};

    /// A trivial real [`Compositor`] that records how many nodes it last saw —
    /// enough to prove [`RenderLoop::tick`] pulls the snapshot and counts it,
    /// without pulling a backend into the core's own tests.
    struct CountingCompositor {
        last_nodes: usize,
    }

    impl Compositor for CountingCompositor {
        fn composite(&mut self, snapshot: &Snapshot) -> CompositeStats {
            self.last_nodes = snapshot.len();
            CompositeStats {
                nodes_composited: snapshot.len(),
                pixels_redrawn: 0,
            }
        }
    }

    /// Each tick pulls the current snapshot, composites it, and accumulates the
    /// frame and node counters.
    #[test]
    fn tick_pulls_snapshot_and_accumulates_counters() {
        let scene = SceneThread::spawn();
        let h = scene.handle();
        h.place_solid(
            SurfaceId(1),
            ClientKey(0),
            Transform::Identity,
            (2, 2),
            0,
            [1, 2, 3, 255],
            true,
        );
        h.place_solid(
            SurfaceId(2),
            ClientKey(0),
            Transform::Translate { dx: 1, dy: 1 },
            (2, 2),
            1,
            [4, 5, 6, 255],
            true,
        );

        let mut render = RenderLoop::new(h.clone(), CountingCompositor { last_nodes: 0 });

        // No presenter attached: tick composites and counts, fires no callbacks.
        render.tick(0);
        assert_eq!(render.counters().frames_produced, 1);
        assert_eq!(render.counters().nodes_composited, 2);
        assert_eq!(render.compositor().last_nodes, 2);

        render.tick(0);
        assert_eq!(render.counters().frames_produced, 2, "counters accumulate");
        assert_eq!(render.counters().nodes_composited, 4);
    }
}
