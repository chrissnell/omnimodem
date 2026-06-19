//! Layer-1 conformance: known-answer tests against published/reference vectors.
//! Each coding block contributes a `#[test]` checked against vectors stored in
//! `tests/vectors/` or inline constants traceable to a named reference source.

use omnimodem_dsp::testutil::{bytes_to_hex, hex_to_bytes};

#[test]
fn harness_links() {
    // Sanity: the testutil surface is reachable from an integration test.
    assert_eq!(bytes_to_hex(&hex_to_bytes("dead")), "dead");
}
