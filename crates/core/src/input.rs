//! The input funnel and the focus routing table (M1 T6).
//!
//! Governing design: `docs/CORE-BOUNDARY.md` §3 (C2: the input pipeline is
//! in-core), §7 (T-input owns input devices and a *read-mostly replica* of the
//! focus routing table), C10 (the temporary focus policy), and
//! `docs/scene_graph_v1.md` §11. Invariant **I-2** governs everything here:
//! input delivery must not wait on rendering.
//!
//! # One funnel, two producers
//!
//! [`InputEvent`] is the single shape every input source produces: the nested
//! winit backend translating desktop events, and the test rig injecting scripted
//! ones. Both hand it to `ProtocolHost::input`, which is a **message into the
//! dispatch thread** — the only thread allowed to touch Wayland objects (§7,
//! unchanged since T2). Nothing here sends a protocol event; this module is
//! pure data plus the geometry needed to decide *who* an event belongs to.
//!
//! The funnel deliberately names **no Smithay type**. Backends depend on this
//! crate and must not learn the protocol library's vocabulary — the same
//! discipline that keeps `Frame` out of the core. Key codes are Linux evdev
//! codes and buttons are `BTN_*` codes, because that is the lingua franca the
//! Wayland protocol itself speaks.
//!
//! # T-input's interface, not yet T-input's thread
//!
//! `CORE-BOUNDARY.md` §7 gives input its own thread. M1 does not have one: in
//! the nested backend winit's event loop must run on the main thread and owns
//! both input intake and presentation, so "T-input" is that loop feeding this
//! funnel. The interface arrives now; the thread arrives with libinput and DRM
//! in **M2**. The deviation is recorded in `docs/scene_graph_v1.md` §11.2 — it
//! is bounded (the funnel is already the seam a real T-input would push into)
//! and it is not silent.

use std::collections::HashMap;

use crate::scene::{Rect, SurfaceId};

/// One input event, in the core's own vocabulary.
///
/// Produced by any input source, consumed by the dispatch thread. Timestamps are
/// monotonic milliseconds, as the Wayland protocol requires; in tests they are
/// supplied by the caller and deterministic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// A key changed state. `code` is a **Linux evdev keycode** (`KEY_A` = 30),
    /// which is exactly what `wl_keyboard.key` carries on the wire.
    Key {
        /// evdev keycode.
        code: u32,
        /// `true` for press, `false` for release.
        pressed: bool,
        /// Event timestamp, monotonic ms.
        time_ms: u32,
    },
    /// The pointer moved to an absolute position in **output coordinates**.
    /// (Relative motion is a later concern — the nested backend gets absolute
    /// positions from winit, and so does the rig.)
    PointerMotion {
        /// X in output pixels.
        x: f64,
        /// Y in output pixels.
        y: f64,
        /// Event timestamp, monotonic ms.
        time_ms: u32,
    },
    /// A pointer button changed state. `button` is a Linux `BTN_*` code
    /// ([`BTN_LEFT`] and friends), as `wl_pointer.button` carries.
    PointerButton {
        /// `BTN_*` code.
        button: u32,
        /// `true` for press, `false` for release.
        pressed: bool,
        /// Event timestamp, monotonic ms.
        time_ms: u32,
    },
    /// Scrolling. Values are in surface-local scroll units (the protocol's
    /// "length of a scroll step" convention); `steps` carries discrete wheel
    /// clicks where the source has them.
    PointerAxis {
        /// Horizontal scroll amount (positive = right).
        horizontal: f64,
        /// Vertical scroll amount (positive = down).
        vertical: f64,
        /// Discrete wheel steps, if the source is a stepped wheel.
        steps: i32,
        /// Event timestamp, monotonic ms.
        time_ms: u32,
    },
}

/// `BTN_LEFT` — the left mouse button's evdev code. Named here so backends and
/// tests need not hard-code the number (and so the one place it is written down
/// is next to the funnel that carries it).
pub const BTN_LEFT: u32 = 0x110;
/// `BTN_RIGHT`.
pub const BTN_RIGHT: u32 = 0x111;
/// `BTN_MIDDLE`.
pub const BTN_MIDDLE: u32 = 0x112;

/// What [`FocusMap::at`] found under a point.
///
/// It carries **both** coordinate facts on purpose. The Wayland protocol shows
/// the client `local` (the cursor's position within its own surface), but the
/// protocol library is told `origin` (where the surface sits in global space)
/// and derives `local` itself. Returning only one invites passing it where the
/// other belongs — a mistake that produces plausible-looking coordinates
/// (`(0, 0)` at every enter) rather than an obvious failure, so the type names
/// both and lets the compiler keep them apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// The surface under the point.
    pub surface: SurfaceId,
    /// That surface's top-left corner, in output coordinates.
    pub origin: (f64, f64),
    /// The point, translated into the surface's own coordinates.
    pub local: (f64, f64),
}

/// Where a mapped surface sits, for input routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FocusEntry {
    /// The surface's extent in output coordinates.
    rect: Rect,
    /// Stacking order — higher is nearer the viewer. With subsurfaces (M2 T7) this
    /// is the surface's index in composition order, so the routing table and the
    /// snapshot agree on "topmost" by construction.
    z: i32,
    /// Whether this surface may take **keyboard** focus. Subsurfaces may not — the
    /// protocol gives them pointer input only, and focus belongs to the window
    /// they are part of. They are still hit-testable, which is why this is a flag
    /// rather than an omission.
    focusable: bool,
}

/// The focus routing table: **a read-mostly replica**, owned by the dispatch
/// thread, of just enough scene geometry to answer "who is under the cursor?"
/// and "who is on top?".
///
/// # Why a replica and not a query
///
/// `CORE-BOUNDARY.md` §7 gives T-input "a read-mostly replica of canonical
/// focus" by name, and this is why: the alternative — asking the scene thread
/// on every pointer motion — is a *synchronous cross-thread round-trip on the
/// input path*, which can queue behind the render thread's snapshot request.
/// That is precisely the coupling **I-2** exists to forbid ("input delivery MUST
/// NOT wait on rendering"). The scene stays canonical (I-5); this is derived,
/// reconstructible state, updated by the same dispatch-thread code that tells
/// the scene about a map, unmap, or resize — so it cannot drift without that
/// code being wrong about what it just published.
///
/// Ordering matches the snapshot's draw order exactly (ascending `z`, ties
/// broken by [`SurfaceId`]), so "topmost" here is the node the compositor
/// painted last. If those two orders ever disagree, input would land on a
/// surface the user cannot see — hence the shared rule and its test.
#[derive(Debug, Default, Clone)]
pub struct FocusMap {
    /// Mapped surfaces only. Unmapped and roleless surfaces are absent, which is
    /// how the T5 rule ("no role, no pixels") extends to input for free: what
    /// cannot be seen cannot be clicked.
    entries: HashMap<SurfaceId, FocusEntry>,
}

impl FocusMap {
    /// An empty table.
    pub fn new() -> Self {
        FocusMap::default()
    }

    /// Record (or update) a mapped surface's extent and stacking order.
    ///
    /// `focusable` is false for subsurfaces: they receive pointer events but never
    /// keyboard focus (the protocol is explicit), so they participate in
    /// [`at`](Self::at) and not in [`topmost`](Self::topmost).
    pub fn map(&mut self, surface: SurfaceId, rect: Rect, z: i32, focusable: bool) {
        self.entries.insert(
            surface,
            FocusEntry {
                rect,
                z,
                focusable,
            },
        );
    }

    /// Forget a surface — unmapped, destroyed, or its client gone. Idempotent.
    pub fn unmap(&mut self, surface: SurfaceId) {
        self.entries.remove(&surface);
    }

    /// Forget every surface. Used when a client disconnects and the dispatch
    /// thread drops its bookkeeping wholesale.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Whether this surface is currently routable (mapped).
    pub fn contains(&self, surface: SurfaceId) -> bool {
        self.entries.contains_key(&surface)
    }

    /// The topmost mapped surface, if any — the C10 keyboard-focus policy in one
    /// line. **Temporary:** "focus follows topmost" is a policy decision, and
    /// policy belongs to the reference policy daemon S1 (M4), not the core
    /// (`CORE-BOUNDARY.md` §4 rule 4). It lives here now because a compositor
    /// with no focus at all cannot be typed into.
    pub fn topmost(&self) -> Option<SurfaceId> {
        self.entries
            .iter()
            .filter(|(_, e)| e.focusable)
            .max_by_key(|(id, e)| (e.z, **id))
            .map(|(id, _)| *id)
    }

    /// The topmost mapped surface containing an output-space point.
    ///
    /// Hit-testing is exact and rectangular in M1 (the only implemented
    /// transform is an integer translation). Declared shapes — `desktop.shape.*`,
    /// which give analytic input regions rather than alpha guesswork — attach
    /// here when they land (M7).
    pub fn at(&self, x: f64, y: f64) -> Option<Hit> {
        self.entries
            .iter()
            .filter(|(_, e)| contains_point(&e.rect, x, y))
            .max_by_key(|(id, e)| (e.z, **id))
            .map(|(id, e)| Hit {
                surface: *id,
                origin: (e.rect.x as f64, e.rect.y as f64),
                local: (x - e.rect.x as f64, y - e.rect.y as f64),
            })
    }
}

/// Whether an output-space point falls inside a rect. Half-open on the far edges
/// (`x < right`), so two abutting windows never both claim the shared column.
fn contains_point(rect: &Rect, x: f64, y: f64) -> bool {
    x >= rect.x as f64 && y >= rect.y as f64 && x < rect.right() as f64 && y < rect.bottom() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SurfaceId = SurfaceId(1);
    const B: SurfaceId = SurfaceId(2);

    /// An empty table focuses nothing and hits nothing — the honest answer when
    /// no window is mapped (and the state the compositor starts in).
    #[test]
    fn empty_map_has_no_focus_and_no_hit() {
        let m = FocusMap::new();
        assert_eq!(m.topmost(), None);
        assert_eq!(m.at(5.0, 5.0), None);
    }

    /// A hit reports coordinates local to the surface, not the output.
    #[test]
    fn hit_reports_surface_local_coordinates() {
        let mut m = FocusMap::new();
        m.map(A, Rect::new(10, 20, 30, 40), 0, true);
        let hit = m.at(15.0, 25.0).expect("inside the surface");
        assert_eq!(hit.surface, A);
        assert_eq!(hit.local, (5.0, 5.0), "point within the surface");
        assert_eq!(hit.origin, (10.0, 20.0), "the surface's own corner");
        assert_eq!(m.at(9.0, 25.0), None, "left of the surface");
        assert_eq!(m.at(15.0, 19.0), None, "above the surface");
    }

    /// Edges are half-open: the far edge belongs to no one, so abutting windows
    /// cannot both claim the boundary pixel.
    #[test]
    fn far_edges_are_exclusive() {
        let mut m = FocusMap::new();
        m.map(A, Rect::new(0, 0, 10, 10), 0, true);
        assert!(m.at(0.0, 0.0).is_some(), "near edge is inclusive");
        assert_eq!(m.at(10.0, 5.0), None, "far x edge is exclusive");
        assert_eq!(m.at(5.0, 10.0), None, "far y edge is exclusive");
    }

    /// Overlapping surfaces resolve by stacking order, and the tie-break matches
    /// the snapshot's (ascending z, then SurfaceId) so input lands on the surface
    /// the compositor drew last — the one the user can actually see.
    #[test]
    fn overlap_resolves_to_the_topmost_and_matches_draw_order() {
        let mut m = FocusMap::new();
        m.map(A, Rect::new(0, 0, 20, 20), 0, true);
        m.map(B, Rect::new(10, 10, 20, 20), 0, true); // same z: higher id is on top
        assert_eq!(m.at(15.0, 15.0).map(|h| h.surface), Some(B), "tie → higher id");
        assert_eq!(m.at(5.0, 5.0).map(|h| h.surface), Some(A), "outside B");

        m.map(A, Rect::new(0, 0, 20, 20), 5, true); // now A is explicitly above B
        assert_eq!(m.at(15.0, 15.0).map(|h| h.surface), Some(A), "higher z wins");
        assert_eq!(m.topmost(), Some(A));
    }

    /// A subsurface is hit-testable but never keyboard-focusable: the protocol
    /// gives focus to the window, and pointer events to whatever is under the
    /// cursor.
    #[test]
    fn subsurfaces_take_pointer_input_but_not_keyboard_focus() {
        let mut m = FocusMap::new();
        m.map(A, Rect::new(0, 0, 20, 20), 0, true); // the window
        m.map(B, Rect::new(0, 0, 10, 10), 1, false); // its decoration, on top

        assert_eq!(
            m.at(5.0, 5.0).map(|h| h.surface),
            Some(B),
            "the pointer lands on the subsurface it is over"
        );
        assert_eq!(
            m.topmost(),
            Some(A),
            "but keyboard focus stays with the window, not its decoration"
        );
    }

    /// Unmapping removes a surface from routing entirely — what cannot be seen
    /// cannot be clicked or focused.
    #[test]
    fn unmapped_surfaces_are_unroutable() {
        let mut m = FocusMap::new();
        m.map(A, Rect::new(0, 0, 20, 20), 0, true);
        m.map(B, Rect::new(0, 0, 20, 20), 1, true);
        assert_eq!(m.topmost(), Some(B));

        m.unmap(B);
        assert_eq!(m.topmost(), Some(A), "focus falls back to what is left");
        assert_eq!(m.at(5.0, 5.0).map(|h| h.surface), Some(A));
        assert!(!m.contains(B));

        m.unmap(A);
        assert_eq!(m.topmost(), None);
        assert_eq!(m.at(5.0, 5.0), None);
    }
}
