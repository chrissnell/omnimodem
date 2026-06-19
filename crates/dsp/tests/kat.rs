//! Layer-1 conformance: known-answer tests against published/reference vectors.
//! Each coding block contributes a `#[test]` checked against vectors stored in
//! `tests/vectors/` or inline constants traceable to a named reference source.
//!
//! This target uses the `testutil` fixtures (AWGN, hex helpers), so it is gated
//! behind the `testutil` feature. Run with `cargo test -p omnimodem-dsp
//! --features testutil`; a plain `cargo test` compiles it to an empty target.
#![cfg(feature = "testutil")]

use omnimodem_dsp::testutil::{bytes_to_hex, hex_to_bytes};

#[test]
fn harness_links() {
    // Sanity: the testutil surface is reachable from an integration test.
    assert_eq!(bytes_to_hex(&hex_to_bytes("dead")), "dead");
}
