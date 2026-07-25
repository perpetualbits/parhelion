//! `Scene` — the canonical scene state, and the protocol→scene message it folds.
//!
//! Governing design: `docs/CORE-BOUNDARY.md` §3 (C4) and `docs/scene_graph_v1.md`.
//! This is the data structure the scene-owner thread owns (see
//! [`crate::scene::thread`]); it has no threading of its own, which keeps it
//! unit-testable in isolation and keeps the ownership story in one place.
//!
//! It absorbs the M0 ledger: the same create/commit/destroy/client-gone
//! lifecycle ([`ProtocolEvent`], grown from the old `LedgerMsg`) folds in here,
//! plus the visual state (placement, size, source) M1 adds. The M0 ledger's unit
//! tests migrated into this module's `tests` (the ledger is gone, its behaviour
//! is not).

use std::collections::HashMap;

use crate::scene::node::{
    ClientKey, NodeRole, SceneNode, SubsurfaceRole, SurfaceId, TextureSource, Transform,
};
use crate::scene::region::{Rect, Region};
use crate::scene::snapshot::{Snapshot, SnapshotDamage, SnapshotNode};

/// How deep a subsurface tree may nest before the scene stops walking.
///
/// Not a protocol limit — the protocol has none, and Smithay's tree happily
/// nests. It is a **cycle and runaway guard** on canonical state that the scene
/// thread walks on every snapshot, every damage calculation, and every hit test:
/// a corrupted parent link must not turn into an unbounded recursion on the one
/// thread that owns the compositor's truth. Sixteen is far past any real toolkit
/// (GTK's deepest decoration nesting is three) and far short of a stack problem.
pub const MAX_SUBSURFACE_DEPTH: usize = 16;

/// A node's own offset, before its parent chain is taken into account.
/// Identity/translate only in M1 (`Transform`).
fn own_offset(node: &SceneNode) -> (i32, i32) {
    match node.transform {
        Transform::Identity => (0, 0),
        Transform::Translate { dx, dy } => (dx, dy),
    }
}

/// The damage a content commit carries, in **surface** coordinates (which equal
/// buffer coordinates in M1 — no scale/transform yet). `Full` is the conservative
/// case: a new buffer with no client damage, or a partial-copy fallback, so the
/// whole surface extent is treated as changed.
#[derive(Debug, Clone)]
pub enum ContentDamage {
    /// The whole surface extent may have changed.
    Full,
    /// Only these surface-local rects changed (translated to output at apply).
    Rects(Vec<Rect>),
}

/// A surface-lifecycle event published by the protocol dispatch thread to the
/// scene owner (`docs/CORE-BOUNDARY.md` §7: the edge is one-directional and
/// asynchronous — the dispatch thread never blocks on a reply, I-3).
///
/// Every variant carries only `Send` core tokens ([`SurfaceId`], [`ClientKey`])
/// so it crosses the thread boundary freely and the scene never sees a protocol
/// object. This is the grown successor to M0's `LedgerMsg`; the visual state
/// (role, geometry, source, title) is *not* here because it is not `Copy` and
/// arrives through the closure setters below instead — the dispatch thread calls
/// them from its shm (T3) and xdg-shell (T5) paths, as do tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolEvent {
    /// A surface was created by the given client.
    SurfaceCreated {
        /// The new surface.
        surface: SurfaceId,
        /// Its owning client, for attribution.
        client: ClientKey,
    },
    /// A surface committed (its pending state became current).
    SurfaceCommitted {
        /// The surface that committed.
        surface: SurfaceId,
    },
    /// A surface was explicitly destroyed (`wl_surface.destroy`).
    SurfaceDestroyed {
        /// The surface that was destroyed.
        surface: SurfaceId,
    },
    /// A client disconnected; all its surfaces are implicitly gone.
    ClientGone {
        /// The client that disconnected.
        client: ClientKey,
    },
}

/// One surface's share of an **effective commit** (M2 T7).
///
/// A commit that becomes effective on a surface with synchronized children makes
/// *all* of their states current at the same instant — that atomicity is
/// user-visible (a window and its decorations must never be seen half-updated).
/// So the protocol side collects the whole subtree's changes into a list of these
/// and sends them as **one** scene message, applied in order by
/// [`Scene::apply_commit`]. One message, one frame's worth of truth.
#[derive(Debug, Clone)]
pub enum SurfaceUpdate {
    /// Place a root's node (output-space) — the C10 cascade at map time.
    Geometry {
        /// The surface being placed.
        surface: SurfaceId,
        /// Output-space offset.
        offset: (i32, i32),
        /// Size in pixels.
        size: (u32, u32),
    },
    /// Position a subsurface relative to its parent.
    Position {
        /// The subsurface.
        surface: SurfaceId,
        /// Parent-relative offset.
        offset: (i32, i32),
    },
    /// A parent's child stacking order, including the parent's own marker.
    Order {
        /// The parent whose order this is.
        parent: SurfaceId,
        /// Children bottom-to-top, with `parent` itself at its slot.
        order: Vec<SurfaceId>,
    },
    /// New content for a surface.
    Content {
        /// The surface.
        surface: SurfaceId,
        /// Its new size.
        size: (u32, u32),
        /// Its new pixels.
        source: TextureSource,
        /// Whether they are fully opaque.
        opaque: bool,
        /// What changed, in surface coordinates.
        damage: ContentDamage,
    },
    /// A surface lost its content (null attach).
    Unmap {
        /// The surface.
        surface: SurfaceId,
    },
}

/// The canonical scene: every live surface's [`SceneNode`], keyed by [`SurfaceId`].
///
/// Rebuilt purely from [`ProtocolEvent`]s (lifecycle) and the setters below
/// (visual state). Applying a lifecycle event is tolerant of the two natural
/// cleanup paths overlapping (an explicit destroy and a client disconnect): a
/// destroy for an already-absent surface is a no-op, exactly as the M0 ledger
/// behaved.
#[derive(Debug, Clone)]
pub struct Scene {
    /// Live surfaces by id.
    nodes: HashMap<SurfaceId, SceneNode>,
    /// Output-space damage accumulated since the last snapshot — the union of
    /// every change (content and structural). Drained into the snapshot. Kept
    /// conservative (only ever grows within a frame; see [`crate::scene::region`]).
    pending_damage: Region,
    /// When set, the next snapshot reports [`SnapshotDamage::Full`] regardless of
    /// `pending_damage` — the first frame and any "don't know" fallback.
    full_damage: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            nodes: HashMap::new(),
            pending_damage: Region::new(),
            // The first frame has no retained output to be incremental against, so
            // it must repaint everything.
            full_damage: true,
        }
    }
}

impl Scene {
    /// A fresh, empty scene (first snapshot damages the whole output).
    pub fn new() -> Self {
        Scene::default()
    }

    // ---- The subsurface tree (M2 T7) ---------------------------------------

    /// A node's offset in **output** coordinates: its own offset plus every
    /// ancestor's, walked to the root.
    ///
    /// A subsurface's `transform` is parent-relative, which is what makes a parent
    /// move carry its whole subtree for free — the children's stored offsets never
    /// change. The walk is bounded by nesting depth, which is a handful even for
    /// toolkits that nest enthusiastically.
    pub fn absolute_offset(&self, surface: SurfaceId) -> (i32, i32) {
        let mut offset = (0, 0);
        let mut current = Some(surface);
        // `seen` guards against a cycle. The protocol cannot create one (a
        // surface may not be its own ancestor), but this walk runs on canonical
        // state that a future bug could corrupt, and an infinite loop in the
        // scene thread would take the compositor with it.
        let mut seen = 0usize;
        while let Some(id) = current {
            let Some(node) = self.nodes.get(&id) else { break };
            let (dx, dy) = own_offset(node);
            offset = (offset.0 + dx, offset.1 + dy);
            current = node.parent;
            seen += 1;
            if seen > MAX_SUBSURFACE_DEPTH {
                break;
            }
        }
        offset
    }

    /// The output-space rectangle a node covers, or an empty rect if it is not
    /// mapped (contributes no pixels, hence no damage).
    pub fn node_rect(&self, surface: SurfaceId) -> Rect {
        if !self.is_mapped(surface) {
            return Rect::new(0, 0, 0, 0);
        }
        let Some(node) = self.nodes.get(&surface) else {
            return Rect::new(0, 0, 0, 0);
        };
        let (ox, oy) = self.absolute_offset(surface);
        Rect::new(ox, oy, node.size.0 as i32, node.size.1 as i32)
    }

    /// **The mapping law, extended through the tree (M2 T7).**
    ///
    /// A node contributes pixels when it has content of its own
    /// ([`SceneNode::is_visible`]) *and* every ancestor does too. A subsurface of
    /// an unmapped window is not "a window that happens to be hidden" — per
    /// protocol it is simply not mapped, and the T5 rule follows it down the tree:
    /// what cannot be seen cannot be clicked.
    pub fn is_mapped(&self, surface: SurfaceId) -> bool {
        let mut current = Some(surface);
        let mut depth = 0usize;
        while let Some(id) = current {
            let Some(node) = self.nodes.get(&id) else {
                return false;
            };
            if !node.is_visible() {
                return false;
            }
            current = node.parent;
            depth += 1;
            if depth > MAX_SUBSURFACE_DEPTH {
                return false;
            }
        }
        true
    }

    /// Make `child` a subsurface of `parent`, placed **above** the parent (the
    /// protocol's default for a new subsurface). Idempotent: re-parenting to the
    /// same parent leaves the existing order alone.
    pub fn attach_subsurface(&mut self, child: SurfaceId, parent: SurfaceId) {
        if !self.nodes.contains_key(&child) || !self.nodes.contains_key(&parent) {
            return;
        }
        self.damage_subtree(child);
        if let Some(node) = self.nodes.get_mut(&child) {
            node.parent = Some(parent);
            node.role = NodeRole::Subsurface(SubsurfaceRole::default());
        }
        if let Some(p) = self.nodes.get_mut(&parent) {
            // The parent's own marker goes in first if the list is empty, so that
            // "above the parent" has something to be above.
            if p.children.is_empty() {
                p.children.push(parent);
            }
            if !p.children.contains(&child) {
                p.children.push(child);
            }
        }
        self.damage_subtree(child);
    }

    /// Replace a parent's child ordering wholesale — the ordered list **including
    /// the parent's own id** as the marker for its place in the stack.
    ///
    /// Taking the whole order rather than diffing `place_above`/`place_below` is
    /// deliberate: the protocol side already holds the authoritative order (it is
    /// Smithay's tree), and re-deriving it here would be a second implementation
    /// of the same list with its own opportunities to disagree. A restack damages
    /// the subtree, since what is visible within those pixels has changed.
    pub fn set_child_order(&mut self, parent: SurfaceId, order: Vec<SurfaceId>) {
        if !self.nodes.contains_key(&parent) {
            return;
        }
        let changed = self
            .nodes
            .get(&parent)
            .map(|p| p.children != order)
            .unwrap_or(false);
        if !changed {
            return;
        }
        self.damage_subtree(parent);
        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children = order;
        }
        self.damage_subtree(parent);
    }

    /// Set a subsurface's position **relative to its parent**. Damages the
    /// subtree's old and new extents (a move takes its own children with it).
    pub fn set_subsurface_position(&mut self, surface: SurfaceId, x: i32, y: i32) {
        // A position that has not changed damages nothing. This matters more than
        // it looks: a subsurface's position is re-stated on **every** effective
        // commit of its parent (that is how the protocol defers it), so damaging
        // unconditionally would repaint every decoration on every keystroke — 76%
        // of the output in the acceptance run, measured, before this check.
        if self
            .nodes
            .get(&surface)
            .map(|n| own_offset(n) == (x, y))
            .unwrap_or(true)
        {
            return;
        }
        let before: Vec<Rect> = self.subtree_rects(surface);
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.transform = Transform::Translate { dx: x, dy: y };
        }
        let after: Vec<Rect> = self.subtree_rects(surface);
        for r in before.into_iter().chain(after) {
            self.damage_rect(r);
        }
    }

    /// Detach a subsurface from its parent — the role object was destroyed while
    /// the `wl_surface` lives on. It becomes a roleless orphan: never displayed.
    pub fn detach_subsurface(&mut self, surface: SurfaceId) {
        self.damage_subtree(surface);
        let parent = self.nodes.get(&surface).and_then(|n| n.parent);
        if let Some(parent) = parent
            && let Some(p) = self.nodes.get_mut(&parent)
        {
            p.children.retain(|id| *id != surface);
        }
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.parent = None;
            node.role = NodeRole::None;
        }
    }

    /// Every rect a subtree currently occupies, root first.
    fn subtree_rects(&self, surface: SurfaceId) -> Vec<Rect> {
        let mut rects = Vec::new();
        self.for_each_in_subtree(surface, &mut |scene, id| {
            rects.push(scene.node_rect(id));
        });
        rects
    }

    /// Damage everything a subtree currently covers.
    fn damage_subtree(&mut self, surface: SurfaceId) {
        for r in self.subtree_rects(surface) {
            self.damage_rect(r);
        }
    }

    /// Walk a subtree (the node and every descendant), depth-first.
    fn for_each_in_subtree(&self, surface: SurfaceId, f: &mut impl FnMut(&Scene, SurfaceId)) {
        self.walk_subtree(surface, 0, f);
    }

    /// Depth-bounded recursion behind [`for_each_in_subtree`].
    fn walk_subtree(&self, surface: SurfaceId, depth: usize, f: &mut impl FnMut(&Scene, SurfaceId)) {
        if depth > MAX_SUBSURFACE_DEPTH {
            return;
        }
        f(self, surface);
        let Some(node) = self.nodes.get(&surface) else {
            return;
        };
        for child in node.children.clone() {
            if child != surface {
                self.walk_subtree(child, depth + 1, f);
            }
        }
    }

    /// Flatten a subtree into composition order, bottom to top, accumulating
    /// offsets — the operation the snapshot is built from.
    ///
    /// The order comes from each node's `children` list, whose self-marker says
    /// where the parent sits among its children. A node with no children is just
    /// itself, which is every surface in the tree until a client builds one.
    fn flatten_subtree(&self, surface: SurfaceId, out: &mut Vec<SurfaceId>, depth: usize) {
        if depth > MAX_SUBSURFACE_DEPTH {
            return;
        }
        let Some(node) = self.nodes.get(&surface) else {
            return;
        };
        if node.children.is_empty() {
            out.push(surface);
            return;
        }
        for child in &node.children {
            if *child == surface {
                out.push(surface);
            } else {
                self.flatten_subtree(*child, out, depth + 1);
            }
        }
    }

    // ---- Damage bookkeeping -------------------------------------------------

    /// Add an output-space rect to the pending frame damage (empty rects ignored).
    fn damage_rect(&mut self, rect: Rect) {
        self.pending_damage.add(rect);
    }

    /// Damage a node's current visible extent **and its subtree's** (no-op if
    /// absent or unmapped). A parent's pixels and its children's are one region
    /// as far as the output is concerned.
    fn damage_node(&mut self, surface: SurfaceId) {
        self.damage_subtree(surface);
    }

    /// This node's visible output rect right now (empty if absent/unmapped).
    fn extent_of(&self, surface: SurfaceId) -> Rect {
        self.node_rect(surface)
    }

    /// Force the next snapshot to full-output damage — the conservative fallback
    /// for any case the scene cannot bound (first frame sets this in `new`).
    pub fn damage_full(&mut self) {
        self.full_damage = true;
    }

    /// Fold one protocol lifecycle event into the scene.
    pub fn apply(&mut self, event: ProtocolEvent) {
        match event {
            ProtocolEvent::SurfaceCreated { surface, client } => {
                self.nodes.insert(surface, SceneNode::new(client));
            }
            ProtocolEvent::SurfaceCommitted { surface } => {
                // create precedes commit in protocol order; if the surface is
                // somehow absent, ignore rather than invent one (ledger parity).
                if let Some(node) = self.nodes.get_mut(&surface) {
                    node.committed = true;
                }
            }
            ProtocolEvent::SurfaceDestroyed { surface } => {
                // Removing a visible surface damages the pixels it occupied.
                self.damage_node(surface);
                self.nodes.remove(&surface);
            }
            ProtocolEvent::ClientGone { client } => {
                // Damage every removed node's extent before dropping them.
                let gone: Vec<SurfaceId> = self
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.client == client)
                    .map(|(id, _)| *id)
                    .collect();
                for id in &gone {
                    let r = self.node_rect(*id);
                    self.damage_rect(r);
                }
                self.nodes.retain(|_, node| node.client != client);
                // Drop dangling child references to the departed surfaces.
                for node in self.nodes.values_mut() {
                    node.children.retain(|id| !gone.contains(id));
                }
            }
        }
    }

    /// Set a node's placement and size (structural change). Damages the union of
    /// the old and new extents, so a move or resize repaints both where it left
    /// and where it landed. No-op if the surface is absent.
    pub fn set_geometry(&mut self, surface: SurfaceId, transform: Transform, size: (u32, u32)) {
        let old = self.extent_of(surface);
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.transform = transform;
            node.size = size;
        }
        let new = self.extent_of(surface);
        self.damage_rect(old);
        self.damage_rect(new);
    }

    /// Bind a node's pixel source and set its opacity. Damages the node's extent
    /// (the whole content is (re)placed — this is the no-client-damage-info path;
    /// the real client path is [`attach_content`](Self::attach_content), which
    /// honours per-commit damage). No-op if absent.
    pub fn set_source(&mut self, surface: SurfaceId, source: TextureSource, opaque: bool) {
        let old = self.extent_of(surface);
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.source = Some(source);
            node.opaque = opaque;
        }
        let new = self.extent_of(surface);
        self.damage_rect(old);
        self.damage_rect(new);
    }

    /// Set a node's size without touching its placement. Damages old ∪ new extent.
    /// No-op if absent.
    pub fn set_size(&mut self, surface: SurfaceId, size: (u32, u32)) {
        let old = self.extent_of(surface);
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.size = size;
        }
        let new = self.extent_of(surface);
        self.damage_rect(old);
        self.damage_rect(new);
    }

    /// Apply a content commit: set size, source, and opacity together, and damage
    /// correctly — the structural-vs-content split (T4). If the extent changed
    /// (map, or a move/resize) it damages old ∪ new extent; otherwise it is a pure
    /// content update and it damages only the client's `damage` rects, translated
    /// from surface to output coordinates and clipped to the extent. This is the
    /// path that makes a small commit redraw a small region. No-op if absent.
    pub fn attach_content(
        &mut self,
        surface: SurfaceId,
        size: (u32, u32),
        source: TextureSource,
        opaque: bool,
        damage: ContentDamage,
    ) {
        let old = self.extent_of(surface);
        let node = match self.nodes.get_mut(&surface) {
            Some(node) => node,
            None => return,
        };
        node.size = size;
        node.source = Some(source);
        node.opaque = opaque;
        let new = self.extent_of(surface);

        if old != new {
            // Map / move / resize: the extent itself changed — repaint both.
            self.damage_rect(old);
            self.damage_rect(new);
        } else {
            // Pure content update at the same extent: honour client damage only.
            match damage {
                ContentDamage::Full => self.damage_rect(new),
                ContentDamage::Rects(rects) => {
                    // Surface origin == output origin of the extent (new.x, new.y).
                    for r in rects {
                        let out = r.translate(new.x, new.y).intersect(&new);
                        self.damage_rect(out);
                    }
                }
            }
        }
    }

    /// Apply one effective commit's worth of updates **atomically** (M2 T7).
    ///
    /// Atomic in the sense that matters: this runs as a single message on the
    /// scene thread, so no snapshot can be taken between a parent's new content
    /// and its children's. A client that moves a window and repositions its
    /// decorations in one commit is never rendered half-moved.
    pub fn apply_commit(&mut self, updates: Vec<SurfaceUpdate>) {
        for update in updates {
            match update {
                SurfaceUpdate::Geometry {
                    surface,
                    offset,
                    size,
                } => self.set_geometry(
                    surface,
                    Transform::Translate {
                        dx: offset.0,
                        dy: offset.1,
                    },
                    size,
                ),
                SurfaceUpdate::Position { surface, offset } => {
                    self.set_subsurface_position(surface, offset.0, offset.1)
                }
                SurfaceUpdate::Order { parent, order } => self.set_child_order(parent, order),
                SurfaceUpdate::Content {
                    surface,
                    size,
                    source,
                    opaque,
                    damage,
                } => self.attach_content(surface, size, source, opaque, damage),
                SurfaceUpdate::Unmap { surface } => self.clear_source(surface),
            }
        }
    }

    /// Clear a node's pixel source — it becomes invisible (contributes no snapshot
    /// node). Damages its old extent (the unmapped pixels must be repainted).
    /// The T3 commit path uses this for a null attach. No-op if absent.
    pub fn clear_source(&mut self, surface: SurfaceId) {
        self.damage_node(surface); // extent while still visible
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.source = None;
            node.opaque = false;
        }
    }

    /// Set a node's role (T5) — which decides whether it may be displayed at all
    /// (see [`NodeRole`]). Damages old ∪ new extent because a role change can
    /// flip visibility in both directions: assigning a role to a surface that
    /// already has a source maps it, and clearing the role back to
    /// [`NodeRole::None`] (an `xdg_toplevel.destroy` that leaves the `wl_surface`
    /// alive) unmaps it. No-op if the surface is absent.
    pub fn set_role(&mut self, surface: SurfaceId, role: NodeRole) {
        let old = self.extent_of(surface);
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.role = role;
        }
        let new = self.extent_of(surface);
        self.damage_rect(old);
        self.damage_rect(new);
    }

    /// Record a toplevel's `xdg_toplevel.set_title`. Pure metadata: nothing in
    /// the core branches on it and it damages nothing. No-op if the surface is
    /// absent or is not a toplevel.
    pub fn set_title(&mut self, surface: SurfaceId, title: Option<String>) {
        if let Some(NodeRole::Toplevel(t)) = self.nodes.get_mut(&surface).map(|n| &mut n.role) {
            t.title = title;
        }
    }

    /// Record a toplevel's `xdg_toplevel.set_app_id`. Pure metadata, as
    /// [`set_title`](Self::set_title). No-op if absent or not a toplevel.
    pub fn set_app_id(&mut self, surface: SurfaceId, app_id: Option<String>) {
        if let Some(NodeRole::Toplevel(t)) = self.nodes.get_mut(&surface).map(|n| &mut n.role) {
            t.app_id = app_id;
        }
    }

    /// Set a node's stacking order (higher composites on top). A restack changes
    /// what is visible within the node's extent, so damage it. No-op if absent.
    pub fn set_z(&mut self, surface: SurfaceId, z: i32) {
        self.damage_node(surface);
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.z = z;
        }
    }

    /// Number of live surfaces.
    pub fn surface_count(&self) -> usize {
        self.nodes.len()
    }

    /// Whether a surface is live.
    pub fn contains(&self, surface: SurfaceId) -> bool {
        self.nodes.contains_key(&surface)
    }

    /// The node for a live surface, if any.
    pub fn get(&self, surface: SurfaceId) -> Option<&SceneNode> {
        self.nodes.get(&surface)
    }

    /// Count of live surfaces owned by a client (attribution check).
    pub fn surface_count_for(&self, client: ClientKey) -> usize {
        self.nodes.values().filter(|n| n.client == client).count()
    }

    /// Build an immutable [`Snapshot`]: a full owned copy of the visible nodes,
    /// sorted back-to-front. Sorting here (once, off the hot path) means the
    /// compositor iterates in draw order with no per-frame sort.
    ///
    /// Ordering is a deterministic total order: ascending `z`, ties broken by
    /// [`SurfaceId`] so two nodes at the same `z` always stack the same way
    /// across runs (goldens depend on it). Persistent structural sharing is
    /// `CORE-BOUNDARY.md` §10.3 and stays an open question (see
    /// [`crate::scene::snapshot`]).
    /// Takes `&mut self` because it **drains** the accumulated damage: the region
    /// reported here is the change since the *previous* snapshot, so once handed
    /// out it must be reset. (The first snapshot, and any `damage_full`, report
    /// [`SnapshotDamage::Full`].)
    pub fn snapshot(&mut self) -> Snapshot {
        // Roots only: a subsurface is composited as part of its parent's tree, in
        // the order that tree dictates, not as an independent node in the z list.
        let mut roots: Vec<(SurfaceId, i32)> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.parent.is_none())
            .map(|(id, n)| (*id, n.z))
            .collect();
        // Back-to-front: ascending z, then ascending SurfaceId as the tiebreak.
        roots.sort_by(|(ida, za), (idb, zb)| za.cmp(zb).then(ida.cmp(idb)));

        // Flatten each root's tree into composition order and keep the nodes that
        // are actually mapped. Order within a tree comes from the children lists;
        // order between trees comes from z — so a subsurface never escapes its
        // parent's place in the stack, which is what the protocol promises.
        let mut nodes = Vec::new();
        for (root, _) in roots {
            let mut flat = Vec::new();
            self.flatten_subtree(root, &mut flat, 0);
            for id in flat {
                if !self.is_mapped(id) {
                    continue;
                }
                let Some(node) = self.nodes.get(&id) else { continue };
                let (dx, dy) = self.absolute_offset(id);
                nodes.push(SnapshotNode {
                    // The snapshot carries **absolute** placement: the renderer
                    // consumes a flat back-to-front list and knows nothing about
                    // trees, exactly as before this task. All the tree semantics
                    // are resolved here, on the scene thread that owns them.
                    transform: if (dx, dy) == (0, 0) {
                        Transform::Identity
                    } else {
                        Transform::Translate { dx, dy }
                    },
                    size: node.size,
                    // is_mapped guarantees Some; clone is cheap (Arc bump for shm).
                    source: node.source.clone().expect("mapped node has a source"),
                    opaque: node.opaque,
                });
            }
        }

        // Drain damage: full for the first frame / fallback, else the accumulated
        // region. Reset so the next snapshot reports only its own changes.
        let damage = if self.full_damage {
            SnapshotDamage::Full
        } else {
            SnapshotDamage::Region(std::mem::take(&mut self.pending_damage))
        };
        self.full_damage = false;
        self.pending_damage = Region::new();

        Snapshot { nodes, damage }
    }

    /// Every mapped surface in composition order, bottom to top — the same order
    /// the snapshot uses, exposed for the input path's hit-testing replica.
    ///
    /// Input and pixels must agree about who is on top; sharing one ordering
    /// function is how that stays true rather than being asserted.
    pub fn composition_order(&self) -> Vec<SurfaceId> {
        let mut roots: Vec<(SurfaceId, i32)> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.parent.is_none())
            .map(|(id, n)| (*id, n.z))
            .collect();
        roots.sort_by(|(ida, za), (idb, zb)| za.cmp(zb).then(ida.cmp(idb)));

        let mut order = Vec::new();
        for (root, _) in roots {
            let mut flat = Vec::new();
            self.flatten_subtree(root, &mut flat, 0);
            order.extend(flat.into_iter().filter(|id| self.is_mapped(*id)));
        }
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::node::ToplevelRole;

    // ----- Migrated M0 ledger lifecycle tests (the ledger died; its behaviour
    // lives on here, now asserted against Scene). -----

    /// Create → commit → destroy walks a surface through its whole lifecycle.
    #[test]
    fn surface_lifecycle() {
        let mut s = Scene::new();
        let (surf, client) = (SurfaceId(1), ClientKey(1));

        s.apply(ProtocolEvent::SurfaceCreated {
            surface: surf,
            client,
        });
        assert_eq!(s.surface_count(), 1);
        assert!(!s.get(surf).unwrap().committed);

        s.apply(ProtocolEvent::SurfaceCommitted { surface: surf });
        assert!(s.get(surf).unwrap().committed);

        s.apply(ProtocolEvent::SurfaceDestroyed { surface: surf });
        assert!(!s.contains(surf));
    }

    /// A client disconnect removes exactly that client's surfaces.
    #[test]
    fn client_gone_removes_only_its_surfaces() {
        let mut s = Scene::new();
        let (a, b) = (ClientKey(1), ClientKey(2));
        s.apply(ProtocolEvent::SurfaceCreated {
            surface: SurfaceId(1),
            client: a,
        });
        s.apply(ProtocolEvent::SurfaceCreated {
            surface: SurfaceId(2),
            client: b,
        });

        s.apply(ProtocolEvent::ClientGone { client: a });
        assert_eq!(s.surface_count(), 1);
        assert_eq!(s.surface_count_for(a), 0);
        assert_eq!(s.surface_count_for(b), 1);
    }

    /// Destroying an unknown surface is a harmless no-op (overlapping cleanup).
    #[test]
    fn destroy_absent_surface_is_noop() {
        let mut s = Scene::new();
        s.apply(ProtocolEvent::SurfaceDestroyed {
            surface: SurfaceId(99),
        });
        assert_eq!(s.surface_count(), 0);
    }

    // ----- New M1 scene behaviour: visibility and the snapshot. -----

    /// A created surface with no geometry/source is live but invisible — it
    /// contributes no snapshot node (a surface is silent until it "attaches").
    /// And, from T5, geometry + source are **not enough**: a roleless surface is
    /// never displayed (the mapping-semantics rule).
    #[test]
    fn created_surface_is_invisible_until_configured() {
        let mut s = Scene::new();
        let surf = SurfaceId(1);
        s.apply(ProtocolEvent::SurfaceCreated {
            surface: surf,
            client: ClientKey(0),
        });
        assert_eq!(s.surface_count(), 1, "surface is live");
        assert!(s.snapshot().is_empty(), "but invisible: no snapshot node");

        s.set_geometry(surf, Transform::Translate { dx: 5, dy: 5 }, (10, 10));
        s.set_source(surf, TextureSource::Solid([1, 2, 3, 255]), true);
        assert!(
            s.snapshot().is_empty(),
            "still invisible: a surface with no role is never displayed (T5)"
        );

        s.set_role(surf, NodeRole::Toplevel(ToplevelRole::default()));
        assert_eq!(s.snapshot().len(), 1, "visible once it also has a role");
    }

    /// The role gate both ways: clearing a mapped toplevel's role back to `None`
    /// (its `xdg_toplevel` destroyed while the `wl_surface` lives on) unmaps it.
    #[test]
    fn clearing_the_role_unmaps() {
        let mut s = Scene::new();
        let surf = SurfaceId(1);
        s.apply(ProtocolEvent::SurfaceCreated {
            surface: surf,
            client: ClientKey(0),
        });
        s.set_role(surf, NodeRole::Toplevel(ToplevelRole::default()));
        s.set_geometry(surf, Transform::Identity, (4, 4));
        s.set_source(surf, TextureSource::Solid([1, 2, 3, 255]), true);
        assert_eq!(s.snapshot().len(), 1);

        s.set_role(surf, NodeRole::None);
        assert!(s.snapshot().is_empty(), "role cleared → unmapped");
        assert_eq!(s.surface_count(), 1, "the node itself is still live");
    }

    /// Title and app_id are recorded on the toplevel role and nothing else
    /// changes — they are metadata, not behaviour (T5).
    #[test]
    fn title_and_app_id_are_recorded_on_the_role() {
        let mut s = Scene::new();
        let surf = SurfaceId(1);
        s.apply(ProtocolEvent::SurfaceCreated {
            surface: surf,
            client: ClientKey(0),
        });
        // Before a role exists the setters are harmless no-ops.
        s.set_title(surf, Some("ignored".into()));
        assert_eq!(s.get(surf).unwrap().role, NodeRole::None);

        s.set_role(surf, NodeRole::Toplevel(ToplevelRole::default()));
        s.set_title(surf, Some("parhelion".into()));
        s.set_app_id(surf, Some("org.parhelion.Test".into()));
        let role = s.get(surf).unwrap().role.toplevel().expect("toplevel role");
        assert_eq!(role.title.as_deref(), Some("parhelion"));
        assert_eq!(role.app_id.as_deref(), Some("org.parhelion.Test"));
    }

    /// The snapshot is sorted back-to-front by z (ascending), ties by SurfaceId.
    #[test]
    fn snapshot_is_sorted_back_to_front() {
        let mut s = Scene::new();
        // Each node gets a distinct red channel = its SurfaceId, so we can read
        // the resulting draw order straight off the snapshot. Insert out of
        // z-order and with a z-tie (ids 20 and 30) to exercise both sort keys.
        for (id, z) in [(SurfaceId(10), 5), (SurfaceId(20), 1), (SurfaceId(30), 1)] {
            s.apply(ProtocolEvent::SurfaceCreated {
                surface: id,
                client: ClientKey(0),
            });
            // Core-injected nodes (as the harness places them): displayable
            // without a client role.
            s.set_role(id, NodeRole::CoreOwned);
            s.set_geometry(id, Transform::Identity, (4, 4));
            s.set_source(id, TextureSource::Solid([id.0 as u8, 0, 0, 255]), true);
            s.set_z(id, z);
        }
        let snap = s.snapshot();
        // Read the marker (red channel) in draw order.
        let order: Vec<u8> = snap
            .nodes
            .iter()
            .map(|n| match n.source {
                TextureSource::Solid([r, ..]) => r,
                _ => unreachable!("only solid sources in this test"),
            })
            .collect();
        // Back-to-front: z=1 id=20, then z=1 id=30 (tie → lower id first),
        // then z=5 id=10 on top.
        assert_eq!(order, vec![20, 30, 10]);
    }

    /// A snapshot is an isolated owned copy: mutating the scene afterwards does
    /// not change a snapshot already taken.
    #[test]
    fn snapshot_is_isolated_from_later_mutation() {
        let mut s = Scene::new();
        let surf = SurfaceId(1);
        s.apply(ProtocolEvent::SurfaceCreated {
            surface: surf,
            client: ClientKey(0),
        });
        s.set_role(surf, NodeRole::CoreOwned);
        s.set_geometry(surf, Transform::Identity, (4, 4));
        s.set_source(surf, TextureSource::Solid([10, 20, 30, 255]), true);

        let snap = s.snapshot();
        // Mutate canonical state after the snapshot was taken.
        s.set_source(surf, TextureSource::Solid([99, 99, 99, 255]), true);
        s.apply(ProtocolEvent::SurfaceDestroyed { surface: surf });

        // The snapshot still reflects the state at capture time.
        assert_eq!(snap.len(), 1);
        assert_eq!(snap.nodes[0].source, TextureSource::Solid([10, 20, 30, 255]));
    }
}
