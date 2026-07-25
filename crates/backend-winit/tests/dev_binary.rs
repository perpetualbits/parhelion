//! T7: the `parhelion-dev` binary's own plumbing — socket, shutdown, cleanup —
//! exercised as a real subprocess, on a machine with no display.
//!
//! Lives in this crate, not the harness, because `CARGO_BIN_EXE_parhelion-dev`
//! is only available to the package that owns the binary — and with it comes
//! cargo's guarantee that the binary exists before the test runs.
//!
//! Governing design: `docs/scene_graph_v1.md` §11.4 and the T6 session summary's
//! open wart (a signal left the socket and its lock file behind). The fix is only
//! believable if the *binary* is what gets signalled, so this spawns it in
//! `--headless` mode — the mode that exists precisely so this test can run in CI.
//!
//! Determinism: waits are on definite conditions (a line on stdout, a file
//! appearing or vanishing, the child exiting) with bounded budgets and loud
//! failures — never a fixed sleep standing in for a condition.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for any single condition before failing loudly.
const BUDGET: Duration = Duration::from_secs(20);

/// Path to the binary under test.
///
/// `CARGO_BIN_EXE_<name>` is set by cargo for integration tests **in the package
/// that owns the binary**, and cargo guarantees the binary is built before the
/// test runs. That guarantee is why this test lives here rather than in the
/// harness crate: there, the path had to be reconstructed from the test
/// executable's own location, and `target/debug/parhelion-dev` only exists if
/// someone happened to have run `cargo build` — true on a developer's machine,
/// false on a fresh CI runner, which is exactly how it failed.
fn dev_binary() -> &'static str {
    env!("CARGO_BIN_EXE_parhelion-dev")
}

/// Spawn `parhelion-dev --headless --socket <path>` and wait until it reports
/// the socket it bound.
///
/// Returns the child **and its stdout reader**, which the caller must keep alive:
/// dropping it closes the pipe, and the child's next `println!` would then fail
/// (Rust panics on a write error to stdout). Holding it also lets the caller read
/// the shutdown line afterwards.
fn spawn_headless(socket: &Path) -> (Child, BufReader<std::process::ChildStdout>) {
    let mut child = Command::new(dev_binary())
        .args(["--headless", "--socket"])
        .arg(socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn parhelion-dev — was it built? (cargo test builds workspace bins)");

    // Read stdout until it announces the display name, so the test proceeds only
    // once the compositor is actually serving.
    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + BUDGET;
    let mut announced = false;
    let mut line = String::new();
    while reader.read_line(&mut line).expect("read parhelion-dev stdout") > 0 {
        if line.contains("WAYLAND_DISPLAY=") {
            announced = true;
            break;
        }
        assert!(
            Instant::now() < deadline,
            "parhelion-dev did not announce a socket within the budget"
        );
        line.clear();
    }
    assert!(
        announced,
        "parhelion-dev did not announce a socket within the budget"
    );
    (child, reader)
}

/// Read whatever the child printed after being signalled, so the test can assert
/// it went through its own shutdown path rather than dying mid-loop.
fn drain_stdout(reader: &mut BufReader<std::process::ChildStdout>) -> String {
    let mut rest = String::new();
    // The child has exited by now, so the pipe is at EOF and this cannot block.
    let _ = std::io::Read::read_to_string(reader, &mut rest);
    rest
}

/// Wait for `path` to exist (or not), or fail loudly.
fn wait_for_existence(path: &Path, want: bool) {
    let deadline = Instant::now() + BUDGET;
    while path.exists() != want {
        assert!(
            Instant::now() < deadline,
            "{path:?} existence never became {want}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The binary serves a real socket headlessly, and a **termination signal leaves
/// no litter**: the socket and its lock file are unlinked because the signal ends
/// the loop through its normal path rather than killing it outright.
///
/// This is the T6 wart, closed and pinned.
#[test]
fn dev_binary_serves_a_socket_and_cleans_up_on_sigterm() {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("wayland-dev-test");
    let lock = dir.path().with_file_name("wayland-dev-test.lock");
    let lock = dir.path().join(lock.file_name().expect("lock name"));

    let (mut child, mut reader) = spawn_headless(&socket);
    wait_for_existence(&socket, true);

    // It is really serving: a connection is accepted.
    UnixStream::connect(&socket).expect("connect to the running compositor");
    assert!(lock.exists(), "the bind took its lock file");

    // SIGTERM → orderly exit → the socket unlinks itself.
    signal_child(&child, libc_sigterm());
    let status = wait_with_budget(&mut child);
    assert!(
        status.success(),
        "parhelion-dev exited cleanly on SIGTERM (status {status:?})"
    );
    assert!(
        drain_stdout(&mut reader).contains("shutting down"),
        "it exited through its own shutdown path, not by dying where it stood"
    );

    wait_for_existence(&socket, false);
    assert!(!socket.exists(), "the socket file was removed on shutdown");
    assert!(!lock.exists(), "and so was its lock file");
}

/// The same for SIGINT — what Ctrl-C in the launching terminal sends, which is
/// how Roland will actually stop it.
#[test]
fn dev_binary_cleans_up_on_sigint() {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("wayland-dev-int");

    let (mut child, _reader) = spawn_headless(&socket);
    wait_for_existence(&socket, true);

    signal_child(&child, libc_sigint());
    let status = wait_with_budget(&mut child);
    assert!(status.success(), "clean exit on SIGINT (status {status:?})");
    wait_for_existence(&socket, false);
}

/// `SIGTERM`'s number on Linux.
fn libc_sigterm() -> i32 {
    15
}

/// `SIGINT`'s number on Linux.
fn libc_sigint() -> i32 {
    2
}

/// Send `signal` to a child we spawned, by its exact PID.
///
/// Uses `kill(1)` rather than a libc binding so the harness gains no new
/// dependency for two calls, and targets the precise PID of a process this test
/// started — never a name or pattern.
fn signal_child(child: &Child, signal: i32) {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(child.id().to_string())
        .status()
        .expect("run kill");
    assert!(status.success(), "signalling the child succeeded");
}

/// Wait for the child to exit within the budget, killing it if it hangs so a
/// failure does not leave a compositor running.
fn wait_with_budget(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + BUDGET;
    loop {
        if let Some(status) = child.try_wait().expect("poll the child") {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("parhelion-dev did not exit within the budget after being signalled");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
