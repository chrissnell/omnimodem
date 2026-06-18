//! Deterministic loopback modem over an authorized UDS — one synthetic
//! "loopback" device, a MockPtt opener, and a file audio backend. It exercises
//! the full gRPC control surface with no hardware, for driving clients like the
//! Go `omnimodem-client` during development.
//!
//! Usage: `cargo run -p omnimodemd --example loopback_server -- [socket-path]`

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sock = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/omnimodem-loopback.sock".to_string());
    let db = std::env::temp_dir().join("omnimodem-loopback.sqlite");
    let _ = std::fs::remove_file(&db);
    eprintln!("loopback modem serving on {sock}");
    omnimodemd::serve_uds_phase2_for_test(std::path::Path::new(&db), std::path::Path::new(&sock)).await
}
