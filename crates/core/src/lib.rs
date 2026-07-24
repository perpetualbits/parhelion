//! Parhelion core process — scene graph, render loop, Wayland protocol.
//!
//! Canonical design document: `docs/CORE-BOUNDARY.md` §3 (the exhaustive list
//! of what runs in-core and why) and §7 (the in-core threading model). Every
//! item added to this crate is subject to invariants I-1..I-12
//! (`docs/CORE-BOUNDARY.md` §5), cited by number in review and tests — in
//! particular the frame path must not block (I-1) and the core must not call a
//! server synchronously (I-3) or run third-party code (I-4).
//!
//! # Contents (M0 → M1-T1)
//!
//! - [`scene`] — the canonical scene graph (C4): node model, [`scene::Scene`],
//!   immutable [`scene::Snapshot`], and the scene-owner thread
//!   ([`scene::SceneThread`]) that owns it (§7). This is I-5's canonical state.
//!   The M0 ledger was absorbed into it.
//! - [`render`] — the render seam ([`render::Compositor`]) and the T-render
//!   skeleton ([`render::RenderLoop`]) that composites snapshots (C5). Carries
//!   no backend type — the concrete pixel target lives in a backend crate.
//! - [`protocol::ProtocolHost`] — the Wayland protocol frontend at
//!   `shards = 1`, per the decision "2026-07-24 — Smithay threading fit". It
//!   owns a dispatch thread (the `Display`) and publishes surface lifecycle to
//!   the scene by message (CORE-BOUNDARY §7, C3).
//!
//! The remaining CORE-BOUNDARY §3 components (DRM/KMS, input, buffers, regime
//! machine, capabilities) arrive across M1..M9 (`docs/parhelion_milestone_plan.md`).

pub mod protocol;
pub mod render;
pub mod scene;
