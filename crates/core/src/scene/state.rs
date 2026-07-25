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

use crate::scene::node::{ClientKey, NodeRole, SceneNode, SurfaceId, TextureSource, Transform};
use crate::scene::region::{Rect, Region};
use crate::scene::snapshot::{Snapshot, SnapshotDamage, SnapshotNode};

/// The output-space rectangle a node covers, or an empty rect if it is not
/// visible (no source or zero area — it contributes no pixels, hence no damage).
/// Placement is `Transform` (identity/translate only in M1); the rect's origin is
/// the node's output offset, which is also the surface→output translation used to
/// map client damage.
fn node_output_rect(node: &SceneNode) -> Rect {
    if !node.is_visible() {
        return Rect::new(0, 0, 0, 0);
    }
    let (ox, oy) = match node.transform {
        Transform::Identity => (0, 0),
        Transform::Translate { dx, dy } => (dx, dy),
    };
    Rect::new(ox, oy, node.size.0 as i32, node.size.1 as i32)
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

    // ---- Damage bookkeeping -------------------------------------------------

    /// Add an output-space rect to the pending frame damage (empty rects ignored).
    fn damage_rect(&mut self, rect: Rect) {
        self.pending_damage.add(rect);
    }

    /// Damage a node's current visible extent (no-op if absent/invisible).
    fn damage_node(&mut self, surface: SurfaceId) {
        if let Some(node) = self.nodes.get(&surface) {
            self.pending_damage.add(node_output_rect(node));
        }
    }

    /// This node's visible output rect right now (empty if absent/invisible).
    fn extent_of(&self, surface: SurfaceId) -> Rect {
        self.nodes
            .get(&surface)
            .map(node_output_rect)
            .unwrap_or(Rect::new(0, 0, 0, 0))
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
                let gone: Vec<Rect> = self
                    .nodes
                    .values()
                    .filter(|n| n.client == client)
                    .map(node_output_rect)
                    .collect();
                for r in gone {
                    self.damage_rect(r);
                }
                self.nodes.retain(|_, node| node.client != client);
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
        let new = node_output_rect(node);

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
        // Collect (SurfaceId, node) for the visible nodes so ties can break by id.
        let mut visible: Vec<(SurfaceId, &SceneNode)> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.is_visible())
            .map(|(id, n)| (*id, n))
            .collect();
        // Back-to-front: ascending z, then ascending SurfaceId as the tiebreak.
        visible.sort_by(|(ida, a), (idb, b)| a.z.cmp(&b.z).then(ida.cmp(idb)));

        let nodes = visible
            .into_iter()
            .map(|(_, n)| SnapshotNode {
                transform: n.transform,
                size: n.size,
                // is_visible guarantees Some; clone is cheap (Arc bump for shm).
                source: n.source.clone().expect("visible node has a source"),
                opaque: n.opaque,
            })
            .collect();

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
