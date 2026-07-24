//! Parhelion `desktop` control-plane dialect — SPINE dialect types and the C7
//! interpreter.
//!
//! Canonical design document: `docs/parhelion_desktop_dialect.md`. This crate
//! is the coder-facing home of CORE-BOUNDARY C7 (the interpolation engine) and
//! satisfies invariants I-6 (the control plane is declarative) and I-12 (ship
//! intent across every boundary) by construction: a submitted SPINE fragment is
//! a description the core executes on its own clock, never a per-frame callback.
//!
//! M0 status: empty skeleton. The five v0.1 types (`prop`, `tween`, `spring`,
//! `gain`, `signal.pointer`) and the dialect-interpreter contract arrive in M3
//! per `docs/parhelion_milestone_plan.md`; this session creates only the crate.
