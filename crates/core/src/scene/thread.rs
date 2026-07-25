//! The scene-owner thread and its handle — where §7 ownership becomes real.
//!
//! Governing design: `docs/CORE-BOUNDARY.md` §7 ("each resource has exactly one
//! owning thread; cross-thread communication is message passing over bounded
//! channels; the scene graph crosses threads only as immutable per-frame
//! snapshots") and `docs/scene_graph_v1.md`.
//!
//! [`Scene`] (the canonical state) is owned by exactly one thread, spawned by
//! [`SceneThread`]. Nothing else touches it directly; every reader and writer
//! goes through a [`SceneHandle`] by message:
//!
//! - the protocol dispatch thread (T-proto[0]) publishes lifecycle events with
//!   [`SceneHandle::emit`] — one-directional, fire-and-forget (I-3: it never
//!   blocks on a reply);
//! - T-render pulls an immutable [`Snapshot`] with [`SceneHandle::snapshot`] —
//!   the scene→render edge; the snapshot is an owned value, so no lock is shared
//!   with the frame path (I-1);
//! - the dispatch thread's xdg-shell path (T5) and tests build visual state with
//!   [`SceneHandle::place_solid`]/[`SceneHandle::mutate`] and read it back with
//!   [`SceneHandle::query`]/[`SceneHandle::wait_until`].
//!
//! Determinism: the request/reply calls ([`snapshot`](SceneHandle::snapshot),
//! [`query`](SceneHandle::query)) round-trip through the single message channel,
//! so after a caller's earlier fire-and-forget sends (same thread, FIFO) the
//! reply observes them applied — no sleeps needed. The one wait on a *future*
//! event ([`wait_until`](SceneHandle::wait_until)) blocks on a definite
//! condition with a timeout, never a fixed delay (mirrors the M0
//! `ProtocolHost::wait_until` it replaces).

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::scene::node::{ClientKey, NodeRole, SurfaceId, TextureSource, Transform};
use crate::scene::snapshot::Snapshot;
use crate::scene::state::{ProtocolEvent, Scene};

/// A message to the scene-owner thread. Private: the world speaks to the scene
/// through [`SceneHandle`]'s typed methods, not by constructing these. Carries
/// boxed closures for the generic read/mutate paths, which keeps the message set
/// tiny (`CLAUDE.md`: no message-variant-per-setter) while the scene stays the
/// sole owner that runs them.
enum SceneMsg {
    /// Fold a protocol lifecycle event (from T-proto[0]) into the scene.
    Protocol(ProtocolEvent),
    /// Run a mutation on the canonical scene (test/future visual-state setters).
    Mutate(Box<dyn FnOnce(&mut Scene) + Send>),
    /// Build a snapshot and send it back — the scene→render edge.
    Snapshot(Sender<Snapshot>),
    /// Run a read against the scene and reply through the closure's own channel.
    Query(Box<dyn FnOnce(&Scene) + Send>),
    /// Reply `true` as soon as `pred` holds, or `false` at `deadline`.
    WaitUntil {
        /// The condition to wait for.
        pred: Box<dyn Fn(&Scene) -> bool + Send>,
        /// Where the result (satisfied / timed-out) is sent.
        reply: Sender<bool>,
        /// When to give up and reply `false`.
        deadline: Instant,
    },
    /// Stop the loop and let the thread exit (sent by [`SceneThread`]'s drop).
    Shutdown,
}

/// A pending [`SceneMsg::WaitUntil`] the scene thread is parked on.
struct Waiter {
    /// The condition being awaited.
    pred: Box<dyn Fn(&Scene) -> bool + Send>,
    /// Where to send the result.
    reply: Sender<bool>,
    /// Timeout instant.
    deadline: Instant,
}

/// A cheap, cloneable handle to the scene-owner thread. All access to canonical
/// scene state goes through one of these; cloning it just clones the channel
/// sender, so T-proto, T-render, and tests each hold their own.
#[derive(Clone)]
pub struct SceneHandle {
    /// The channel into the scene thread. Fire-and-forget sends ignore the error
    /// (the thread may be gone during teardown); request/reply sends expect a
    /// live thread because a caller is waiting on the answer.
    tx: Sender<SceneMsg>,
}

impl SceneHandle {
    /// Publish a protocol lifecycle event to the scene (T-proto[0] → T-scene).
    /// Fire-and-forget and best-effort: a send failure means the scene thread is
    /// shutting down, which is fine to drop (I-3: the publisher never blocks).
    pub fn emit(&self, event: ProtocolEvent) {
        let _ = self.tx.send(SceneMsg::Protocol(event));
    }

    /// Run a mutation on the canonical scene. Fire-and-forget; ordering with
    /// later reads is guaranteed by the single FIFO channel. Used by tests and
    /// (from T3/T5) by the code that turns buffers and xdg state into geometry.
    pub fn mutate(&self, f: impl FnOnce(&mut Scene) + Send + 'static) {
        let _ = self.tx.send(SceneMsg::Mutate(Box::new(f)));
    }

    /// Convenience for tests: create a fully-configured, committed, visible solid
    /// node in one message — the **scene-injected** path that bypasses the
    /// protocol entirely.
    ///
    /// The node is given [`NodeRole::CoreOwned`], which is what makes it
    /// displayable without a client role (T5's mapping rule): this stands in for
    /// the C10 fallback family the core will place itself (solid decorations, the
    /// "server crashed" surface). Client content now arrives the honest way —
    /// through xdg-shell — so this is no longer a stand-in for that path.
    #[allow(clippy::too_many_arguments)] // a test convenience; the fields are the node's, not an abstraction to factor.
    pub fn place_solid(
        &self,
        surface: SurfaceId,
        client: ClientKey,
        transform: Transform,
        size: (u32, u32),
        z: i32,
        color: [u8; 4],
        opaque: bool,
    ) {
        self.mutate(move |s| {
            s.apply(ProtocolEvent::SurfaceCreated { surface, client });
            s.apply(ProtocolEvent::SurfaceCommitted { surface });
            s.set_role(surface, NodeRole::CoreOwned);
            s.set_geometry(surface, transform, size);
            s.set_source(surface, TextureSource::Solid(color), opaque);
            s.set_z(surface, z);
        });
    }

    /// Take an immutable snapshot of the current visible scene — the value
    /// T-render composites. Round-trips through the scene thread, so it reflects
    /// every earlier send from this thread (FIFO). Panics only if the scene
    /// thread has died (a caller is genuinely waiting on this).
    pub fn snapshot(&self) -> Snapshot {
        let (tx, rx) = channel();
        self.tx
            .send(SceneMsg::Snapshot(tx))
            .expect("scene thread alive for snapshot");
        rx.recv().expect("scene thread replied with snapshot")
    }

    /// Run a read `f` against the canonical scene and return its result. The
    /// closure runs on the scene thread (the only place the `Scene` is legal to
    /// read); the value comes back over a reply channel.
    pub fn query<R: Send + 'static>(&self, f: impl FnOnce(&Scene) -> R + Send + 'static) -> R {
        let (tx, rx) = channel();
        self.tx
            .send(SceneMsg::Query(Box::new(move |s| {
                let _ = tx.send(f(s));
            })))
            .expect("scene thread alive for query");
        rx.recv().expect("scene thread replied to query")
    }

    /// Block until `pred` holds on the scene or `timeout` elapses; returns
    /// whether it was satisfied. Used where there is no round-trip to synchronise
    /// on — e.g. observing a client disconnect, whose `ClientGone` arrives on the
    /// scene thread asynchronously. Waits on the definite condition, never a
    /// fixed sleep.
    pub fn wait_until(
        &self,
        timeout: Duration,
        pred: impl Fn(&Scene) -> bool + Send + 'static,
    ) -> bool {
        let (tx, rx) = channel();
        let deadline = Instant::now() + timeout;
        self.tx
            .send(SceneMsg::WaitUntil {
                pred: Box::new(pred),
                reply: tx,
                deadline,
            })
            .expect("scene thread alive for wait_until");
        // Err only if the scene thread dropped the reply (e.g. superseded waiter
        // or shutdown) — treat as "condition not met".
        rx.recv().unwrap_or(false)
    }
}

/// Owns the scene thread: spawns it on construction, and on drop asks it to stop
/// and joins it, so the canonical state is torn down deterministically (crash-
/// only teardown ordering, like [`crate::protocol::ProtocolHost`]).
pub struct SceneThread {
    /// The handle callers clone from.
    handle: SceneHandle,
    /// The owning thread, joined on drop.
    join: Option<JoinHandle<()>>,
}

impl SceneThread {
    /// Spawn the scene-owner thread with an empty [`Scene`] and return its owner.
    pub fn spawn() -> Self {
        let (tx, rx) = channel();
        let join = std::thread::Builder::new()
            .name("parhelion-scene".into())
            .spawn(move || run_scene(rx))
            .expect("spawn scene thread");
        SceneThread {
            handle: SceneHandle { tx },
            join: Some(join),
        }
    }

    /// A fresh handle to this scene (cheap clone of the channel).
    pub fn handle(&self) -> SceneHandle {
        self.handle.clone()
    }
}

impl Default for SceneThread {
    fn default() -> Self {
        Self::spawn()
    }
}

impl Drop for SceneThread {
    fn drop(&mut self) {
        // Best-effort stop, then join so the thread (and the Scene it owns) is
        // gone before we return.
        let _ = self.handle.tx.send(SceneMsg::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The scene thread body: own the [`Scene`], apply messages in order, and park
/// on at most one pending [`Waiter`] at a time (sufficient for M1; a second
/// wait supersedes the first, whose caller then reads `false`).
fn run_scene(rx: Receiver<SceneMsg>) {
    let mut scene = Scene::new();
    let mut waiter: Option<Waiter> = None;

    loop {
        // Receive the next message. With a waiter parked, bound the wait by its
        // deadline so a never-satisfied condition still times out.
        let msg = match &waiter {
            Some(w) => {
                let now = Instant::now();
                if now >= w.deadline {
                    let w = waiter.take().expect("waiter present");
                    let _ = w.reply.send((w.pred)(&scene));
                    continue;
                }
                match rx.recv_timeout(w.deadline - now) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        let w = waiter.take().expect("waiter present");
                        let _ = w.reply.send((w.pred)(&scene));
                        continue;
                    }
                    // All handles dropped: nothing more can arrive; exit.
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(m) => m,
                Err(_) => break,
            },
        };

        match msg {
            SceneMsg::Protocol(event) => scene.apply(event),
            SceneMsg::Mutate(f) => f(&mut scene),
            SceneMsg::Snapshot(reply) => {
                let _ = reply.send(scene.snapshot());
            }
            SceneMsg::Query(f) => f(&scene),
            SceneMsg::WaitUntil {
                pred,
                reply,
                deadline,
            } => {
                if pred(&scene) {
                    let _ = reply.send(true);
                } else {
                    // Supersede any existing waiter (its caller reads `false`).
                    waiter = Some(Waiter {
                        pred,
                        reply,
                        deadline,
                    });
                }
            }
            SceneMsg::Shutdown => break,
        }

        // A state change may have satisfied the parked waiter.
        if let Some(w) = &waiter
            && (w.pred)(&scene)
        {
            let w = waiter.take().expect("waiter present");
            let _ = w.reply.send(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::snapshot::SnapshotNode;

    /// place_solid → the node is visible in a snapshot with the expected source.
    #[test]
    fn place_solid_then_snapshot() {
        let scene = SceneThread::spawn();
        let h = scene.handle();
        h.place_solid(
            SurfaceId(1),
            ClientKey(0),
            Transform::Translate { dx: 2, dy: 2 },
            (4, 4),
            0,
            [10, 20, 30, 255],
            true,
        );
        let snap = h.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap.nodes[0].source, TextureSource::Solid([10, 20, 30, 255]));
    }

    /// emit lifecycle events → query reads the resulting surface count.
    #[test]
    fn query_reads_surface_count() {
        let scene = SceneThread::spawn();
        let h = scene.handle();
        h.emit(ProtocolEvent::SurfaceCreated {
            surface: SurfaceId(1),
            client: ClientKey(0),
        });
        h.emit(ProtocolEvent::SurfaceCreated {
            surface: SurfaceId(2),
            client: ClientKey(0),
        });
        assert_eq!(h.query(|s| s.surface_count()), 2);
    }

    /// A snapshot taken before a mutation is isolated from it, across the thread
    /// boundary: the in-flight snapshot keeps its captured value while canonical
    /// state moves on. A fresh snapshot reflects the new state.
    #[test]
    fn snapshot_isolated_across_threads() {
        let scene = SceneThread::spawn();
        let h = scene.handle();
        h.place_solid(
            SurfaceId(1),
            ClientKey(0),
            Transform::Identity,
            (4, 4),
            0,
            [10, 20, 30, 255],
            true,
        );
        let captured: Snapshot = h.snapshot();

        // Mutate canonical state after capturing.
        h.mutate(|s| s.set_source(SurfaceId(1), TextureSource::Solid([99, 99, 99, 255]), true));
        h.emit(ProtocolEvent::SurfaceDestroyed {
            surface: SurfaceId(1),
        });

        // Captured snapshot is unchanged; a fresh one reflects the destroy.
        let expected = SnapshotNode {
            transform: Transform::Identity,
            size: (4, 4),
            source: TextureSource::Solid([10, 20, 30, 255]),
            opaque: true,
        };
        assert_eq!(captured.nodes, vec![expected]);
        assert!(h.snapshot().is_empty(), "fresh snapshot sees the destroy");
    }

    /// wait_until returns true immediately when the condition already holds, and
    /// false when it never becomes true within the timeout. (The "becomes true
    /// via a later message" path is covered by the ProtocolHost disconnect
    /// integration test, which drives a real socket close.)
    #[test]
    fn wait_until_immediate_and_timeout() {
        let scene = SceneThread::spawn();
        let h = scene.handle();
        // Empty scene: count==0 holds now.
        assert!(h.wait_until(Duration::from_secs(5), |s| s.surface_count() == 0));
        // A condition that never holds times out to false, quickly.
        assert!(!h.wait_until(Duration::from_millis(50), |s| s.surface_count() == 5));
    }
}
