//! Damage regions — the conservative rect algebra that lets the compositor stop
//! repainting unchanged pixels (M1 T4).
//!
//! Governing design: `docs/scene_graph_v1.md` (the damage section). This is a
//! small, owned rect algebra with three load-bearing rules:
//!
//! - **Over-approximation is always legal; under-approximation is a bug class.**
//!   Every operation here rounds *outward* — a union keeps every covered pixel, a
//!   clip only ever shrinks to the clip bounds. Damage may make the renderer do
//!   *more* work than strictly necessary, never less, because "incremental must
//!   equal from-scratch" (the governing property) only survives conservative
//!   damage.
//! - **No subtraction.** Within a frame, damage only grows; we never remove area
//!   from a region. Subtraction is where region code grows teeth (splitting rects,
//!   fragment explosions), and M1 needs none of it.
//! - **Bounded.** A [`Region`] never holds more than [`MAX_DAMAGE_RECTS`] rects;
//!   past that it coalesces to its bounding box. A client posting thousands of
//!   scattered damage rects cannot make the compositor's damage bookkeeping itself
//!   unbounded — it just costs precision (a bigger redraw), which is safe.
//!
//! Deliberately *not* using a region crate or Smithay's desktop-layer region
//! handling; this owns exactly the ops T4 needs (union / translate / intersect /
//! clip / coalesce) and keeps the snapshot — and so the backend — free of any
//! Smithay geometry type.

/// Maximum rects a [`Region`] keeps before collapsing to its bounding box.
///
/// The trade is precision vs. bounded bookkeeping. Real content damage is a
/// handful of rects (a cursor, a line of text, a couple of dirty tiles), so 16
/// keeps the common case exact. Past it, a region that would otherwise grow
/// without bound (a pathological many-small-rects client, constraint 1) collapses
/// to one bounding box — over-approximate but O(1) to carry and still correct.
/// The number is a cost knob, not a correctness one: any value ≥ 1 is sound.
pub const MAX_DAMAGE_RECTS: usize = 16;

/// A rectangle in pixel coordinates (output space for scene/frame damage, surface
/// space at the protocol boundary before translation).
///
/// The origin is signed, so a rect may sit partly or wholly off one edge (the
/// compositor clips); `w`/`h` are treated as non-negative extents (a zero or
/// negative extent is an [empty](Self::is_empty) rect that covers nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge (may be negative).
    pub x: i32,
    /// Top edge (may be negative).
    pub y: i32,
    /// Width in pixels (non-negative; ≤ 0 means empty).
    pub w: i32,
    /// Height in pixels (non-negative; ≤ 0 means empty).
    pub h: i32,
}

impl Rect {
    /// A rect at `(x, y)` with extent `(w, h)`.
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }

    /// The exclusive right edge, `x + w` (saturating).
    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }

    /// The exclusive bottom edge, `y + h` (saturating).
    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    /// Whether this rect covers no pixels (non-positive extent).
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// Pixel area (0 for an empty rect). Used for the pixels-redrawn counter, so
    /// it is a `usize` that never underflows.
    pub fn area(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.w as usize * self.h as usize
        }
    }

    /// This rect shifted by `(dx, dy)`.
    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            w: self.w,
            h: self.h,
        }
    }

    /// The overlap of two rects, as a rect (empty if they do not overlap). This is
    /// the one intersection T4 uses — clipping damage to an extent or the frame.
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        Rect {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0),
            h: (y1 - y0).max(0),
        }
    }

    /// The smallest rect containing both (their bounding box). Empty operands are
    /// ignored so a bbox of "something and nothing" is that something.
    pub fn bounding(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// A conservative damage region: a bounded list of rects whose union is the set of
/// pixels that may have changed. Empty rects are never stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Region {
    /// Covering rects (their union is the region). Invariant: none is empty, and
    /// `rects.len() <= MAX_DAMAGE_RECTS`.
    rects: Vec<Rect>,
}

impl Region {
    /// The empty region — covers nothing.
    pub fn new() -> Self {
        Region::default()
    }

    /// A region covering exactly `rect` (empty if `rect` is empty).
    pub fn from_rect(rect: Rect) -> Self {
        let mut r = Region::new();
        r.add(rect);
        r
    }

    /// Whether the region covers no pixels.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// The covering rects (their union is the region).
    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    /// Number of covering rects (a damage-rects counter reads this).
    pub fn len(&self) -> usize {
        self.rects.len()
    }

    /// Total covered area as a sum of rect areas. This is an **upper bound** on
    /// distinct pixels when rects overlap (we never subtract), which is exactly
    /// what the pixels-redrawn counter wants — a conservative ceiling.
    pub fn area(&self) -> usize {
        self.rects.iter().map(Rect::area).sum()
    }

    /// The bounding box of the whole region, or an empty rect if the region is
    /// empty.
    pub fn bounding_box(&self) -> Rect {
        let mut acc = Rect::new(0, 0, 0, 0);
        for r in &self.rects {
            acc = acc.bounding(r);
        }
        acc
    }

    /// Add a rect (union it in). Empty rects are ignored. If the list would exceed
    /// [`MAX_DAMAGE_RECTS`], the region coalesces to its bounding box first — the
    /// bounded-precision rule.
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        self.rects.push(rect);
        if self.rects.len() > MAX_DAMAGE_RECTS {
            self.coalesce();
        }
    }

    /// Union `other` into this region (rect by rect, so the coalesce threshold is
    /// respected throughout).
    pub fn union(&mut self, other: &Region) {
        for r in &other.rects {
            self.add(*r);
        }
    }

    /// Shift the whole region by `(dx, dy)` — surface→output translation.
    pub fn translate(&mut self, dx: i32, dy: i32) {
        for r in &mut self.rects {
            *r = r.translate(dx, dy);
        }
    }

    /// Clip every rect to `bounds`, dropping any that fall entirely outside. Used
    /// to confine client damage to a surface's extent (and, at render, the frame).
    pub fn clip(&mut self, bounds: Rect) {
        self.rects.retain_mut(|r| {
            *r = r.intersect(&bounds);
            !r.is_empty()
        });
    }

    /// Collapse the region to a single rect: its bounding box. The bounded-list
    /// fallback (over-approximate but O(1) to carry).
    fn coalesce(&mut self) {
        let bbox = self.bounding_box();
        self.rects.clear();
        if !bbox.is_empty() {
            self.rects.push(bbox);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_intersect_and_area() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
        assert_eq!(a.intersect(&b).area(), 25);
        // Disjoint → empty.
        let c = Rect::new(100, 100, 4, 4);
        assert!(a.intersect(&c).is_empty());
        assert_eq!(a.intersect(&c).area(), 0);
    }

    #[test]
    fn rect_translate_and_bounding() {
        let a = Rect::new(2, 3, 4, 5);
        assert_eq!(a.translate(10, -1), Rect::new(12, 2, 4, 5));
        let b = Rect::new(0, 0, 1, 1);
        // Bounding box spans both.
        assert_eq!(a.bounding(&b), Rect::new(0, 0, 6, 8));
        // Bounding with empty is identity.
        assert_eq!(a.bounding(&Rect::new(0, 0, 0, 0)), a);
    }

    #[test]
    fn region_union_and_clip() {
        let mut r = Region::from_rect(Rect::new(0, 0, 4, 4));
        r.union(&Region::from_rect(Rect::new(10, 10, 4, 4)));
        assert_eq!(r.len(), 2);
        assert_eq!(r.area(), 32);
        // Clip to a window that only catches the first rect (partly).
        r.clip(Rect::new(0, 0, 2, 8));
        assert_eq!(r.rects(), &[Rect::new(0, 0, 2, 4)]);
    }

    #[test]
    fn region_ignores_empty_rects() {
        let mut r = Region::new();
        r.add(Rect::new(5, 5, 0, 10)); // zero width
        r.add(Rect::new(5, 5, 10, -1)); // negative height
        assert!(r.is_empty());
    }

    /// The pathological many-small-rects case: past the threshold the region
    /// collapses to a single bounding box, staying bounded while still covering
    /// every added rect (over-approximation is legal).
    #[test]
    fn region_coalesces_past_threshold() {
        let mut r = Region::new();
        // Add far more than the threshold, scattered on a diagonal.
        for i in 0..(MAX_DAMAGE_RECTS as i32 + 50) {
            r.add(Rect::new(i * 10, i * 10, 2, 2));
        }
        // Never exceeds the cap; here it has collapsed to one bbox.
        assert!(r.len() <= MAX_DAMAGE_RECTS);
        let bbox = r.bounding_box();
        // The bbox still covers the first and last rects (nothing was dropped).
        assert!(bbox.intersect(&Rect::new(0, 0, 2, 2)).area() > 0);
        let last = (MAX_DAMAGE_RECTS as i32 + 49) * 10;
        assert!(bbox.intersect(&Rect::new(last, last, 2, 2)).area() > 0);
    }
}
