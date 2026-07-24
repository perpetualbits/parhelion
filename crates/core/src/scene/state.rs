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

use crate::scene::node::{ClientKey, SceneNode, SurfaceId, TextureSource, Transform};
use crate::scene::snapshot::{Snapshot, SnapshotNode};

/// A surface-lifecycle event published by the protocol dispatch thread to the
/// scene owner (`docs/CORE-BOUNDARY.md` §7: the edge is one-directional and
/// asynchronous — the dispatch thread never blocks on a reply, I-3).
///
/// Every variant carries only `Send` core tokens ([`SurfaceId`], [`ClientKey`])
/// so it crosses the thread boundary freely and the scene never sees a protocol
/// object. This is the grown successor to M0's `LedgerMsg`; the visual state
/// (geometry, source) is *not* here because in M1 it comes from tests standing
/// in for T3/T5, not from the wire (see the setters below).
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
#[derive(Debug, Default, Clone)]
pub struct Scene {
    /// Live surfaces by id.
    nodes: HashMap<SurfaceId, SceneNode>,
}

impl Scene {
    /// A fresh, empty scene.
    pub fn new() -> Self {
        Scene::default()
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
                self.nodes.remove(&surface);
            }
            ProtocolEvent::ClientGone { client } => {
                self.nodes.retain(|_, node| node.client != client);
            }
        }
    }

    /// Set a node's placement and size. No-op if the surface is absent — the
    /// callers are core/test code (not hostile input), and a setter racing a
    /// destroy is harmless. In M1 this stands in for the geometry T3/T5 will
    /// derive from buffers and xdg-shell.
    pub fn set_geometry(&mut self, surface: SurfaceId, transform: Transform, size: (u32, u32)) {
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.transform = transform;
            node.size = size;
        }
    }

    /// Bind a node's pixel source and set its opacity. No-op if absent.
    pub fn set_source(&mut self, surface: SurfaceId, source: TextureSource, opaque: bool) {
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.source = Some(source);
            node.opaque = opaque;
        }
    }

    /// Set a node's size without touching its placement. The T3 commit path uses
    /// this: a `wl_shm` buffer defines the surface's pixel size, but its position
    /// is a separate concern (test setters now, xdg-shell in T5). No-op if absent.
    pub fn set_size(&mut self, surface: SurfaceId, size: (u32, u32)) {
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.size = size;
        }
    }

    /// Clear a node's pixel source — it becomes invisible (contributes no
    /// snapshot node). The T3 commit path uses this for a null attach
    /// (`wl_surface.attach(null)` then commit), which unmaps the content per
    /// protocol. No-op if absent.
    pub fn clear_source(&mut self, surface: SurfaceId) {
        if let Some(node) = self.nodes.get_mut(&surface) {
            node.source = None;
            node.opaque = false;
        }
    }

    /// Set a node's stacking order (higher composites on top). No-op if absent.
    pub fn set_z(&mut self, surface: SurfaceId, z: i32) {
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
    pub fn snapshot(&self) -> Snapshot {
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
        Snapshot { nodes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(s.snapshot().len(), 1, "visible once geometry+source set");
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
