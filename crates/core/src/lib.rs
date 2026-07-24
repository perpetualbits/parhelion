//! Parhelion core process — scene graph, render loop, Wayland protocol.
//!
//! Canonical design document: `docs/CORE-BOUNDARY.md` §3 (the exhaustive list
//! of what runs in-core and why) and §7 (the in-core threading model). Every
//! item added to this crate is subject to invariants I-1..I-12
//! (`docs/CORE-BOUNDARY.md` §5), cited by number in review and tests — in
//! particular the frame path must not block (I-1) and the core must not call a
//! server synchronously (I-3) or run third-party code (I-4).
//!
//! M0 status: empty skeleton. The core is built up from M1 onward per
//! `docs/parhelion_milestone_plan.md`; this session creates only the crate.
