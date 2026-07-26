//! **T-render** on metal: the same render loop, paced by the display engine
//! instead of by a test (M2 T1).
//!
//! Governing docs: `docs/CORE-BOUNDARY.md` §7 (T-render), `docs/scene_graph_v1.md`
//! §4 and §13.
//!
//! # What changed, and what deliberately did not
//!
//! Nothing about the render loop itself. It still pulls an immutable `Snapshot`,
//! hands it to the CPU compositor, and notifies the protocol side that a frame
//! was presented — the same [`RenderLoop::tick`] the headless golden tests call.
//! What changed is **who calls it**: M1's tick came from whoever owned the loop
//! (a test, or winit's redraw), and here it comes from a vblank on T-commit.
//!
//! That is the whole of "the render tick is driven by real vblank events". The
//! headless tick semantics are untouched, which is why the entire existing suite
//! still proves what it proved before.
//!
//! # Frame-path obligations (I-1)
//!
//! This thread blocks in exactly one place: waiting for the next tick. That wait
//! *is* the frame clock — the compositor is supposed to be idle between vblanks —
//! and it is a channel receive, not a lock shared with another thread and not a
//! synchronous request to anyone (I-3). The snapshot it composites is an owned
//! value, so the scene thread is never blocked by rendering either.

use std::sync::mpsc;
use std::time::Instant;

use parhelion_backend_headless::composite::CpuCompositor;
use parhelion_core::render::RenderLoop;
use smithay::reexports::calloop::channel::Sender;

use crate::commit::{Presented, Tick};
use crate::present;

/// Run T-render until T-commit goes away.
///
/// Each turn: wait for a tick (which carries a recycled pixel buffer), produce a
/// frame, convert it into the scanout format, and hand the bytes back. Exactly
/// one frame is ever in flight, because T-commit ticks only after a vblank.
pub(crate) fn run(
    mut render: RenderLoop<CpuCompositor>,
    ticks: mpsc::Receiver<Tick>,
    frames: Sender<Presented>,
    start: Instant,
) {
    // Monotonic milliseconds from a fixed base — what `wl_callback.done` and
    // every input event timestamp are measured in.
    while let Ok(mut scratch) = ticks.recv() {
        let time_ms = start.elapsed().as_millis() as u32;
        render.tick(time_ms);
        present::frame_to_xrgb8888(render.compositor().frame(), &mut scratch);
        if frames.send(scratch).is_err() {
            // T-commit is gone; there is nowhere to put pixels.
            break;
        }
    }
}
