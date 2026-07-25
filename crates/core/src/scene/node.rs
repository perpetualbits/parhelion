//! Scene node model — the per-surface data the canonical scene graph holds.
//!
//! Governing design: `docs/CORE-BOUNDARY.md` §3 (C4: the scene graph is the
//! canonical state — node tree, transforms, texture-source bindings) and
//! `docs/scene_graph_v1.md` (this subsystem's canonical doc: the node model and
//! the 3D-ready / 2.5D-implemented line). Governed by invariant I-5 (this data
//! *is* the canonical state) and Thesis 1/3 (`docs/VISION.md`).
//!
//! # Born 3D-ready, implemented 2.5D (Thesis 1 / Thesis 3)
//!
//! Every node carries a [`Transform`] slot and a [`TextureSource`] binding from
//! day one — the structural commitment that a window is the *degenerate case* of
//! a 3D object, not the only case (Thesis 1). But in M1 only the axis-aligned
//! integer-translation path is implemented, composited, or tested (Thesis 3):
//! [`Transform`] defaults to identity/translation and anything else is a hard
//! rejection at composite time, never a half-working path. Building 3D now is
//! scope creep; building types that *forbid* 3D would be a Thesis-1 violation —
//! the vocabulary is open, the implementation is narrow.

use std::sync::Arc;

/// A core-assigned surface identifier — the key under which the scene holds a
/// node. Assigned by the protocol dispatch thread when a surface is created and
/// the only surface handle that crosses the channel into the scene (never a
/// borrowed `WlSurface`; decision "2026-07-24 — Smithay threading fit",
/// requirement 2). Carried forward unchanged from the absorbed M0 ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SurfaceId(pub u64);

/// A core-assigned client identifier, assigned when a client is admitted. Used
/// for attribution (which client owns a surface) and bulk cleanup on disconnect.
/// Opaque; not a Wayland `ClientId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientKey(pub u64);

/// A node's placement in the scene.
///
/// # The 3D-ready / 2.5D-implemented seam
///
/// This is the [`node`](self)-level expression of Thesis 3. The enum is
/// *extensible* — the future variants (a general affine 2D transform, a full
/// 4×4 for true 3D objects, perspective) attach here — but in M1 only
/// [`Transform::Identity`] and [`Transform::Translate`] are constructible and
/// composited. The CPU compositor rejects any other variant with an
/// `unimplemented!`-class panic rather than silently mis-rendering it: a
/// half-working 3D path is worse than an honest "not yet". When a real transform
/// arrives it lands with its own composited, golden-tested path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    /// No offset — the node sits at the output origin. Equivalent to
    /// `Translate { dx: 0, dy: 0 }`; kept distinct as the explicit default.
    Identity,
    /// Integer pixel translation — the only *moving* transform implemented in
    /// M1. `dx`/`dy` are signed so a node may be positioned partly or wholly
    /// off-screen (the compositor clips); this is the "axis-aligned integer
    /// position path" Thesis 3 calls sacred.
    Translate {
        /// Horizontal offset in output pixels (may be negative).
        dx: i32,
        /// Vertical offset in output pixels (may be negative).
        dy: i32,
    },
    // Future (Thesis 1, unimplemented in M1 — the compositor rejects them):
    //   Affine2 { .. }      — rotation/scale/shear in the output plane
    //   Matrix(Mat4)        — a true 3D-transformed object
    // These are named here so the extension point is visible; adding one is a
    // new variant plus its composited path, not a redesign.
}

impl Default for Transform {
    /// A node with no explicit placement sits at the origin.
    fn default() -> Self {
        Transform::Identity
    }
}

/// A block of decoded, tightly-packed RGBA8 pixels — the CPU-accessible pixel
/// data a [`TextureSource`] can carry, shared cheaply by [`Arc`].
///
/// **Source-neutral on purpose.** This is where a `wl_shm` buffer's contents land
/// after the dispatch thread copies and decodes them (T3), but the type says
/// nothing about *where* the pixels came from — that is the seam rule below made
/// concrete. A future dmabuf that gets read back to the CPU, or a Rayland replay
/// that produces CPU pixels, would populate the same block; only the copy path
/// (in the protocol frontend) knows the origin. The renderer blits this and asks
/// no questions.
///
/// `rgba` is exactly `width * height * 4` bytes, row-major, top-left origin, no
/// stride padding (the copy path strips the buffer's stride) — matching the
/// `Frame` layout the compositor writes into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelBuffer {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA8 pixels, row-major, `width * height * 4` bytes, tightly packed.
    pub rgba: Vec<u8>,
}

/// Where a node's pixels come from — **the Rayland seam**.
///
/// This binding is the single point at which the scene graph and renderer decide
/// what a node is textured with. It is deliberately extensible, and the load-
/// bearing rule lives here:
///
/// > **Nothing in the scene or renderer may assume pixels are locally produced.**
///
/// A node does not know whether its texture came from a local shared-memory
/// buffer, a local GPU dmabuf, or a Rayland replay service rendering on behalf of
/// a board across the network (`docs/VISION.md` Thesis 1; `CORE-BOUNDARY.md` C9).
/// M1 ships two members: [`TextureSource::Solid`] (tests + the C10 fallback
/// family) and [`TextureSource::Shm`] (T3 — a `wl_shm` buffer copied into a
/// source-neutral [`PixelBuffer`]). The future members — `Dmabuf`, and the
/// Rayland **token-buffer** source (`CORE-BOUNDARY.md` C9) — attach here; keeping
/// them out of the type today is scope discipline, not a closed door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureSource {
    /// A solid RGBA colour, `[r, g, b, a]`. The M1 test source and the substrate
    /// of the built-in C10 fallbacks (solid decorations, "server crashed"
    /// surface). Composited by the CPU compositor directly, no buffer import.
    Solid([u8; 4]),
    /// A decoded shared-memory buffer's pixels (`wl_shm`, T3). The dispatch
    /// thread copies the client's `argb8888`/`xrgb8888` buffer into this
    /// source-neutral [`PixelBuffer`] at commit and releases the `wl_buffer`
    /// immediately; the renderer blits the block knowing nothing about "shm"
    /// (the seam rule above). Shared by [`Arc`] so snapshots are cheap.
    Shm(Arc<PixelBuffer>),
    // Future sources (attach here; the seam rule above governs all of them):
    //   Dmabuf(..)        — local GPU buffer import (M2)
    //   RaylandToken(..)  — token-buffer S-side source (CORE-BOUNDARY C9)
}

/// The xdg-shell toplevel role's metadata (T5): what the client calls itself.
///
/// Captured into canonical state (I-5) because it is the core's truth about the
/// window, and the future policy daemon (S1, M4) and debug inspectors read it
/// over the control plane. **No behaviour hangs off these in M1** — they are
/// recorded and inspectable, nothing more; placement, focus, and matching rules
/// that would consume them are policy, and policy is not in the core (§4 rule 4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToplevelRole {
    /// `xdg_toplevel.set_title`, if the client has set one.
    pub title: Option<String>,
    /// `xdg_toplevel.set_app_id`, if the client has set one.
    pub app_id: Option<String>,
}

/// What a node *is* — and therefore whether it may be displayed at all (T5).
///
/// # The mapping rule (the T5 migration)
///
/// Wayland is explicit: **a `wl_surface` without a role is never displayed.**
/// Until T5 the scene composited any surface that had committed pixels, which
/// was convenient for tests and non-conformant; from T5 the role gates
/// visibility ([`SceneNode::is_visible`]), and "mapped" needs no separate flag —
/// a toplevel is mapped exactly when it carries the role *and* has a committed
/// source, which is precisely what unmap (null attach) and destroy clear.
///
/// [`CoreOwned`](NodeRole::CoreOwned) is the deliberate exception: content the
/// core itself puts in the scene (the C10 fallback family — solid decorations,
/// the "server crashed" surface — and the harness's scene-injected fixtures)
/// never travels through the protocol and so can carry no client role, yet must
/// still be displayable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NodeRole {
    /// No role: a bare `wl_surface`. Live in the scene, never displayed. This is
    /// the state every surface starts in.
    #[default]
    None,
    /// An xdg-shell toplevel (T5). Displayed once it has a committed source.
    Toplevel(ToplevelRole),
    /// Core-injected content that bypasses the protocol entirely: C10 fallbacks
    /// and harness fixtures. Always displayable (no client role to wait for).
    CoreOwned,
}

impl NodeRole {
    /// Whether a node carrying this role may contribute pixels *at all* — the
    /// role half of [`SceneNode::is_visible`]. A roleless surface never can.
    pub fn displays(&self) -> bool {
        !matches!(self, NodeRole::None)
    }

    /// The toplevel metadata, if this is a toplevel role. Read by the rig (and,
    /// later, by the control plane) — nothing in the core branches on it.
    pub fn toplevel(&self) -> Option<&ToplevelRole> {
        match self {
            NodeRole::Toplevel(t) => Some(t),
            _ => None,
        }
    }
}

/// One surface's canonical scene state.
///
/// Holds both the lifecycle facts absorbed from the M0 ledger (owning
/// [`ClientKey`], whether it has committed) and the visual state the scene graph
/// adds in M1 (role, placement, size, source, opacity). A node is *visible* — and
/// so appears in a [`Snapshot`](crate::scene::Snapshot) — only once it has a
/// display-worthy [`NodeRole`], a source, and a non-empty size; a freshly-created
/// surface has none of those and is live in the scene but contributes no pixels,
/// exactly as a real client's surface is silent until it takes a role and
/// attaches a buffer.
///
/// No longer `Copy`: a node's [`TextureSource`] may hold an `Arc<PixelBuffer>`
/// (shm), so the node is `Clone` (a cheap ref-count bump for the pixels) but not
/// trivially copyable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneNode {
    /// The client that created this surface (attribution + bulk cleanup).
    pub client: ClientKey,
    /// Whether the surface has committed at least once (absorbed ledger fact).
    pub committed: bool,
    /// The surface's role, which gates whether it is ever displayed (T5). A
    /// freshly-created surface has [`NodeRole::None`] and stays invisible no
    /// matter what it commits.
    pub role: NodeRole,
    /// Placement in the output. Identity/translation only in M1 (see [`Transform`]).
    pub transform: Transform,
    /// Node size in pixels, `(width, height)`. `(0, 0)` until geometry is set —
    /// a zero-area node contributes nothing to a snapshot.
    pub size: (u32, u32),
    /// Stacking order: higher `z` composites later (on top). Ties break by
    /// [`SurfaceId`] for a deterministic total order (see the snapshot builder).
    pub z: i32,
    /// The node's pixel source, or `None` until one is bound. `None` nodes are
    /// live but invisible (see the type docs).
    pub source: Option<TextureSource>,
    /// Whether the node is fully opaque. Drives the future opaque-region /
    /// occlusion work (damage is T4); in M1 it only informs the compositor's
    /// blend (opaque = overwrite).
    pub opaque: bool,
}

impl SceneNode {
    /// A freshly-created surface: owned by `client`, not yet committed, roleless,
    /// at the origin, zero-size, no source. Invisible until it takes a role and a
    /// source lands.
    pub fn new(client: ClientKey) -> Self {
        SceneNode {
            client,
            committed: false,
            role: NodeRole::None,
            transform: Transform::default(),
            size: (0, 0),
            z: 0,
            source: None,
            opaque: false,
        }
    }

    /// Whether this node contributes pixels to a frame: it has a display-worthy
    /// role (T5 — a roleless surface is never shown), a source, and a non-empty
    /// area. Invisible nodes are skipped by the snapshot builder.
    ///
    /// For a toplevel, role + source *is* the definition of "mapped": the role
    /// arrives with `xdg_surface.get_toplevel` and the source with the first
    /// buffer commit, and unmap clears the source again.
    pub fn is_visible(&self) -> bool {
        self.role.displays() && self.source.is_some() && self.size.0 > 0 && self.size.1 > 0
    }
}
