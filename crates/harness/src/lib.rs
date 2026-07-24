//! Parhelion test harness — headless golden-screenshot and protocol test rigs.
//!
//! Governing docs: `docs/parhelion_milestone_plan.md` M0 (this rig is the
//! project's velocity multiplier — nothing lands without a headless-verifiable
//! test) and `docs/CORE-BOUNDARY.md` §8 (the crash-only failure semantics the
//! protocol rig exercises). No standalone design document yet; the harness
//! earns one when its shape is settled.
//!
//! M0 status: empty skeleton. The golden-test rig (render → hash/compare with
//! per-pixel tolerance) and the protocol rig (scripted Wayland test client) are
//! later M0 tasks, each with its own prompt; this session creates only the
//! crate.
