//! Checks that `scripts/sandbox-test.sh` really isolates the run it wraps.
//!
//! The script sets `GWI_SANDBOXED=1` inside the sandbox; the checks are skipped
//! without it, so a plain `cargo test` is unaffected.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

fn sandboxed() -> bool {
    let on = std::env::var_os("GWI_SANDBOXED").is_some();
    if !on {
        eprintln!("GWI_SANDBOXED is unset: skipping (run scripts/sandbox-test.sh)");
    }
    on
}

/// An address in TEST-NET-1 (RFC 5737): never routable, so outside the sandbox a
/// connect on a machine with a network times out, while a sandbox refuses at once.
#[test]
fn external_network_is_refused_not_merely_slow() {
    if !sandboxed() {
        return;
    }
    let external: SocketAddr = "192.0.2.1:80".parse().unwrap();
    let err = TcpStream::connect_timeout(&external, Duration::from_secs(5)).unwrap_err();
    assert_ne!(
        err.kind(),
        ErrorKind::TimedOut,
        "the connect was attempted on the network, so the sandbox is not blocking it: {err}"
    );
}

/// The wiremock and MCP stdio tests need loopback, so the sandbox must keep it.
#[test]
fn loopback_stays_reachable() {
    if !sandboxed() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
}
