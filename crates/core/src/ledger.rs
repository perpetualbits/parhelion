//! The scene ledger — the minimal receiving side of the protocol→scene edge.
//!
//! Governing design: `docs/CORE-BOUNDARY.md` §7 (the scene owner receives
//! changes from the protocol dispatch thread by message, never by shared
//! state) and the decision-log entry "2026-07-24 — Smithay threading fit"
//! (requirement 3: a minimal ledger of live surfaces, inspectable by the
//! harness).
//!
//! # This is NOT the scene graph
//!
//! It tracks only *which surfaces are live and whether they have committed*,
//! keyed by core-assigned [`SurfaceId`] and attributed to a core-assigned
//! [`ClientKey`]. There is deliberately **no geometry, no buffers, no stacking,
//! no roles, no transforms** — nothing M1's real scene graph (C4) would have to
//! fight or migrate. M1 replaces this wholesale. Its only jobs today are to
//! prove the §7 message edge works and to give the protocol rig something to
//! assert on.
//!
//! # No Wayland types cross into here
//!
//! Every field and message is a core-assigned token or a plain bool — there is
//! no `wayland-server` type in this module's API. That is the same isolation
//! property the threading spike demonstrated (the scene side had no Wayland
//! type in scope): the ledger cannot accidentally depend on protocol state.

use std::collections::HashMap;

/// A core-assigned surface identifier. Assigned by the protocol dispatch thread
/// when a surface is created, and the only surface handle that crosses the
/// channel to the ledger — never a borrowed `WlSurface` (decision 2026-07-24,
/// requirement 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SurfaceId(pub u64);

/// A core-assigned client identifier, assigned when a client is admitted. Used
/// purely for attribution — proving the ledger does not confuse two clients
/// sharing one dispatch shard. Opaque; not a Wayland `ClientId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientKey(pub u64);

/// A message published by the protocol dispatch thread to the ledger owner.
///
/// Every variant carries only `Send` core tokens (`SurfaceId`, `ClientKey`) or
/// nothing — so it crosses a thread boundary freely and the ledger never sees a
/// protocol object (I-3: the edge is one-directional and asynchronous; the
/// dispatch thread never blocks on a reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerMsg {
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

/// What the ledger knows about one live surface. Minimal by design (see the
/// module docs) — a commit flag and an owner, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceRecord {
    /// The client that created this surface.
    pub client: ClientKey,
    /// Whether the surface has committed at least once.
    pub committed: bool,
}

/// The set of live surfaces, rebuilt purely from [`LedgerMsg`]s.
///
/// Inspectable by the harness through the accessors below. Applying a message
/// is idempotent enough to tolerate the two natural cleanup paths overlapping
/// (an explicit `wl_surface.destroy` and a client disconnect): a destroy for an
/// already-absent surface is a no-op.
#[derive(Debug, Default, Clone)]
pub struct Ledger {
    /// Live surfaces by id.
    surfaces: HashMap<SurfaceId, SurfaceRecord>,
}

impl Ledger {
    /// A fresh, empty ledger.
    pub fn new() -> Self {
        Ledger::default()
    }

    /// Fold one message into the ledger.
    pub fn apply(&mut self, msg: LedgerMsg) {
        match msg {
            LedgerMsg::SurfaceCreated { surface, client } => {
                self.surfaces.insert(
                    surface,
                    SurfaceRecord {
                        client,
                        committed: false,
                    },
                );
            }
            LedgerMsg::SurfaceCommitted { surface } => {
                // A commit for a surface we do not know is impossible in
                // protocol order (create precedes commit), so trust it; if the
                // surface is somehow absent, ignore rather than invent one.
                if let Some(rec) = self.surfaces.get_mut(&surface) {
                    rec.committed = true;
                }
            }
            LedgerMsg::SurfaceDestroyed { surface } => {
                self.surfaces.remove(&surface);
            }
            LedgerMsg::ClientGone { client } => {
                self.surfaces.retain(|_, rec| rec.client != client);
            }
        }
    }

    /// Number of live surfaces.
    pub fn surface_count(&self) -> usize {
        self.surfaces.len()
    }

    /// Whether a surface is live.
    pub fn contains(&self, surface: SurfaceId) -> bool {
        self.surfaces.contains_key(&surface)
    }

    /// The record for a live surface, if any.
    pub fn get(&self, surface: SurfaceId) -> Option<&SurfaceRecord> {
        self.surfaces.get(&surface)
    }

    /// Count of live surfaces owned by a given client (attribution check).
    pub fn surface_count_for(&self, client: ClientKey) -> usize {
        self.surfaces
            .values()
            .filter(|rec| rec.client == client)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create → commit → destroy walks a surface through its whole lifecycle.
    #[test]
    fn surface_lifecycle() {
        let mut l = Ledger::new();
        let (s, c) = (SurfaceId(1), ClientKey(1));

        l.apply(LedgerMsg::SurfaceCreated {
            surface: s,
            client: c,
        });
        assert_eq!(l.surface_count(), 1);
        assert!(!l.get(s).unwrap().committed);

        l.apply(LedgerMsg::SurfaceCommitted { surface: s });
        assert!(l.get(s).unwrap().committed);

        l.apply(LedgerMsg::SurfaceDestroyed { surface: s });
        assert!(!l.contains(s));
    }

    /// A client disconnect removes exactly that client's surfaces.
    #[test]
    fn client_gone_removes_only_its_surfaces() {
        let mut l = Ledger::new();
        let (a, b) = (ClientKey(1), ClientKey(2));
        l.apply(LedgerMsg::SurfaceCreated {
            surface: SurfaceId(1),
            client: a,
        });
        l.apply(LedgerMsg::SurfaceCreated {
            surface: SurfaceId(2),
            client: b,
        });

        l.apply(LedgerMsg::ClientGone { client: a });
        assert_eq!(l.surface_count(), 1);
        assert_eq!(l.surface_count_for(a), 0);
        assert_eq!(l.surface_count_for(b), 1);
    }

    /// Destroying an unknown surface is a harmless no-op (overlapping cleanup).
    #[test]
    fn destroy_absent_surface_is_noop() {
        let mut l = Ledger::new();
        l.apply(LedgerMsg::SurfaceDestroyed {
            surface: SurfaceId(99),
        });
        assert_eq!(l.surface_count(), 0);
    }
}
