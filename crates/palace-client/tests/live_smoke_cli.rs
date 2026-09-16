//! Integration tests for the `live-smoke` binary.
//!
//! `live-smoke` is not part of the hermetic suite — it exists to drive the real
//! runtime against a real server. It has no argument parser and no `--help`, so
//! its only offline-observable behaviour is the environment-driven configuration
//! echo printed before it dials, and the fact that a refused connection is
//! handled without a crash. That is exactly what this test asserts.
//!
//! Everything past the echo needs a live pserver and is deliberately not
//! exercised: the test points the binary at a closed loopback port so the
//! connection fails immediately, and stops asserting after the config line and
//! the exit status.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "live-smoke-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A port nothing is listening on, so `connect` is refused with no DNS traffic.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

#[test]
fn env_configuration_is_echoed_and_a_refused_connection_exits_successfully() {
    let home = TempDir::new();
    let port = closed_port();

    let output = Command::new(env!("CARGO_BIN_EXE_live-smoke"))
        .env("PALACE_HOST", "127.0.0.1")
        .env("PALACE_PORT", port.to_string())
        .env("PALACE_USER", "SmokeTester")
        .env("PALACE_SMOKE_SECS", "0")
        .env("PALACE_SMOKE_ROOM", "817")
        // Isolate from the real user's home/corpus and from any optional modes a
        // developer may have exported into the environment.
        .env("HOME", home.0.as_os_str())
        .env_remove("PALACE_SMOKE_VIEWPORTS")
        .env_remove("PALACE_RUN_SCRIPT")
        .env_remove("PALACE_CLICK_ROOM")
        .env_remove("PALACE_DEBUG_FRAMES")
        .output()
        .expect("spawn live-smoke");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "live-smoke must exit 0 even when the server is unreachable; \
         status={:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );

    let expected = format!("live-smoke: 127.0.0.1:{port} as \"SmokeTester\", 0s, goto 817");
    assert!(
        stdout.contains(&expected),
        "expected {expected:?} in:\n{stdout}"
    );

    // Past the echo the runtime reports the failed connection offline rather than
    // crashing, which is what makes this a smoke test and not a unit test.
    assert!(
        stdout.contains("[status]"),
        "runtime status events should be reported:\n{stdout}"
    );
}
