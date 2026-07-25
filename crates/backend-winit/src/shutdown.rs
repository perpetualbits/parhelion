//! Graceful shutdown on SIGTERM/SIGINT (M1 T7).
//!
//! # Why this exists
//!
//! `parhelion-dev` binds a Wayland socket, and the listening socket unlinks
//! itself (and its `.lock`) when it is dropped. A signal with no handler kills
//! the process outright, so `Drop` never runs and the socket file is left behind:
//! T6 shipped exactly that wart, and this is where it comes due.
//!
//! # The mechanism, and why this one
//!
//! A signal handler may do almost nothing safely — it runs between two arbitrary
//! instructions and can observe any lock in any state. So the handler here does
//! the smallest possible thing: set an atomic flag. The event loop notices the
//! flag on its next turn and exits **through its normal path**, which drops the
//! `ProtocolHost`, which stops the dispatch thread, which drops the socket, which
//! unlinks the files. Nothing is cleaned up *in* the handler; the handler only
//! asks the program to end the way it already knows how.
//!
//! Two signals are handled and both mean the same thing here: `SIGINT` (Ctrl-C in
//! the terminal that launched it) and `SIGTERM` (a supervisor or `kill`).
//! `SIGKILL` cannot be caught, so a hard kill still leaves the socket behind —
//! that is the kernel's contract, not a gap in ours, and wayland-server's lock
//! protocol makes a stale socket harmless on the next bind.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A flag set when a termination signal arrives.
///
/// Cheap to poll (one relaxed atomic load per event-loop turn) and cloneable, so
/// the loop and anything else that wants to stop can share one.
#[derive(Debug, Clone, Default)]
pub struct ShutdownFlag {
    /// Set by the signal handler; never cleared.
    raised: Arc<AtomicBool>,
}

impl ShutdownFlag {
    /// A flag that has not been raised.
    pub fn new() -> Self {
        ShutdownFlag::default()
    }

    /// Whether a termination signal has arrived.
    pub fn is_raised(&self) -> bool {
        self.raised.load(Ordering::Relaxed)
    }

    /// Raise it by hand — used by tests, and by any caller that wants the same
    /// orderly exit without a signal.
    pub fn raise(&self) {
        self.raised.store(true, Ordering::Relaxed);
    }

    /// Arrange for `SIGINT` and `SIGTERM` to raise this flag.
    ///
    /// Returns an error if the handlers cannot be installed, which the caller
    /// should report rather than swallow: a compositor that will not shut down
    /// cleanly is worth knowing about *before* it litters someone's runtime dir.
    pub fn install_signal_handlers(&self) -> std::io::Result<()> {
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            signal_hook::flag::register(signal, Arc::clone(&self.raised))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh flag is down; raising it by hand puts it up. (The by-hand path is
    /// the one the loop's own "close requested" handling can reuse.)
    #[test]
    fn raise_sets_the_flag() {
        let flag = ShutdownFlag::new();
        assert!(!flag.is_raised());
        flag.raise();
        assert!(flag.is_raised());
    }

    /// Clones share one flag — the loop polls its copy, the handler sets another.
    #[test]
    fn clones_share_state() {
        let a = ShutdownFlag::new();
        let b = a.clone();
        b.raise();
        assert!(a.is_raised(), "raising one clone raises them all");
    }

    /// The real thing: install the handlers, send this process a SIGTERM, and
    /// watch the flag go up. This is the whole mechanism — handler sets flag,
    /// loop reads flag — exercised for real rather than described.
    #[test]
    fn a_real_signal_raises_the_flag() {
        let flag = ShutdownFlag::new();
        flag.install_signal_handlers()
            .expect("install signal handlers");
        assert!(!flag.is_raised());

        signal_hook::low_level::raise(signal_hook::consts::SIGTERM).expect("raise SIGTERM");

        // The handler runs synchronously on this thread for a directed signal, so
        // the flag is up by the time `raise` returns.
        assert!(flag.is_raised(), "SIGTERM raised the shutdown flag");
    }
}
