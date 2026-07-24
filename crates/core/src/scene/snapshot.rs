//! The immutable per-frame snapshot — the only form in which the scene graph
//! crosses a thread boundary (`docs/CORE-BOUNDARY.md` §7).
//!
//! # Why a snapshot exists
//!
//! Canonical scene state is owned by exactly one thread (the scene owner; §7).
//! T-render must not read that state under a shared lock — that would put a lock
//! between a frame-path thread and the scene thread, which I-1 forbids. Instead
//! the scene owner produces an immutable [`Snapshot`] — a self-contained,
//! owned value — and hands it to the render side. Once produced, a snapshot is
//! frozen: later mutations of canonical state cannot touch an in-flight frame.
//! That property is what the snapshot-isolation test asserts.
//!
//! # Representation (M1) and the open question
//!
//! Snapshot v1 is a **full copy** of the visible-node list, sorted back-to-front.
//! For the handful of nodes M1 composites this is trivially cheap, and it is the
//! most obviously-correct thing. Persistent structural sharing (copy-on-write of
//! an unchanged subtree) is `CORE-BOUNDARY.md` §10.3 and stays an **open
//! question** — deliberately not built here (`CLAUDE.md`: no abstraction beyond
//! what the task requires). When snapshot-assembly cost shows up in a benchmark,
//! §10.3 is reopened with evidence; until then a `Vec` copy is correct and clear.

use crate::scene::node::{TextureSource, Transform};

/// One visible node, flattened for compositing. Carries only what the compositor
/// needs — placement, size, source, opacity — not the lifecycle bookkeeping
/// (owner, committed) the canonical [`SceneNode`](crate::scene::SceneNode) holds.
/// `source` is non-optional here because invisible (source-less) nodes are
/// filtered out during snapshot assembly.
///
/// Not `Copy`: [`TextureSource`] may carry an `Arc<PixelBuffer>` (shm). Cloning a
/// snapshot node is still cheap — the pixel block is shared by ref-count, never
/// deep-copied — so a full-copy [`Snapshot`] does not duplicate pixel data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotNode {
    /// Placement in the output (identity/translation only in M1).
    pub transform: Transform,
    /// Node size in pixels, `(width, height)`; always non-empty for a snapshot node.
    pub size: (u32, u32),
    /// The node's pixel source.
    pub source: TextureSource,
    /// Whether the node is fully opaque (compositor blend hint).
    pub opaque: bool,
}

/// An immutable, owned, back-to-front-ordered list of the scene's visible nodes.
///
/// "Back-to-front" means `nodes[0]` is drawn first (furthest back) and the last
/// element is drawn last (on top) — the painter's-algorithm order the CPU
/// compositor iterates directly, so ordering lives here (built once) rather than
/// in the hot compositing loop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Visible nodes, sorted back-to-front (ascending `z`, ties by `SurfaceId`).
    pub nodes: Vec<SnapshotNode>,
}

impl Snapshot {
    /// An empty snapshot — no visible nodes. Compositing it clears the frame.
    pub fn empty() -> Self {
        Snapshot::default()
    }

    /// Number of visible nodes this snapshot will composite.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the snapshot has no visible nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
