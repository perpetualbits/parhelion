//! The scene graph — Parhelion's canonical state (`docs/CORE-BOUNDARY.md` C4).
//!
//! Canonical document: `docs/scene_graph_v1.md`. This module is the in-core
//! truth about what surfaces exist, where they are, and what textures them —
//! the data invariant I-5 names when it says "canonical state lives in the
//! core". Everything else (servers, the render side) holds only views derived
//! from here.
//!
//! # Structure
//!
//! - [`node`] — the per-surface node model: [`SceneNode`], [`Transform`],
//!   [`TextureSource`], and the id tokens [`SurfaceId`]/[`ClientKey`]. This is
//!   where the *born-3D-ready, implemented-2.5D* line (Thesis 1/3) and the
//!   Rayland texture-source seam live.
//! - [`state`] — [`Scene`], the canonical map, and [`ProtocolEvent`], the
//!   lifecycle message the protocol dispatch thread folds in (absorbing the M0
//!   ledger).
//! - [`snapshot`] — [`Snapshot`]/[`SnapshotNode`], the immutable per-frame value
//!   that is the *only* way the scene crosses to T-render (§7).
//! - [`thread`] — [`SceneThread`]/[`SceneHandle`], the ownership wrapper: the
//!   canonical state lives on one dedicated thread, reached only by message.
//!
//! # Ownership (CORE-BOUNDARY §7)
//!
//! ```text
//!   T-proto[0] ──ProtocolEvent──▶ ┌─────────────┐
//!   (dispatch)   (lifecycle msgs) │  T-scene    │  owns Scene (canonical state)
//!                                 │ (SceneThread)│
//!   test/core ──setters/queries──▶└─────┬───────┘
//!                                       │ Snapshot (immutable, owned)
//!                                       ▼
//!                                 ┌─────────────┐
//!                                 │  T-render   │  RenderLoop::tick() composites
//!                                 └─────────────┘
//! ```
//!
//! The scene crosses to render *only* as an immutable [`Snapshot`]; no lock is
//! ever shared between the scene thread and a frame-path thread (I-1). The
//! proto→scene edge is one-directional and async (I-3).

pub mod node;
pub mod region;
pub mod snapshot;
pub mod state;
pub mod thread;

pub use node::{
    ClientKey, NodeRole, PixelBuffer, SceneNode, SurfaceId, TextureSource, ToplevelRole, Transform,
};
pub use region::{Rect, Region, MAX_DAMAGE_RECTS};
pub use snapshot::{Snapshot, SnapshotDamage, SnapshotNode};
pub use state::{ContentDamage, ProtocolEvent, Scene};
pub use thread::{SceneHandle, SceneThread};
