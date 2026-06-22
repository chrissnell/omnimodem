//! Group C — FEC & coding (soft-decision throughout). `Llr` is the spine.
//!
//! All blocks document and test bit order explicitly (LSB-first on the AX.25
//! wire, MSB-first big-endian for WSJT-X). Decoders consume the locked `Llr`
//! convention `L = ln(P(0)/P(1))`: positive ⇒ bit 0, hard slice `bit = (L < 0)`.

pub mod crc;
pub mod nrzi;
pub mod gray;
pub mod scramble;
pub mod rs;
pub mod llr;
pub mod ldpc;
pub mod osd;
pub mod slicer;

// Phase 5 (WS-A) building-block groups: the convolutional/Viterbi/Fano/FHT/
// interleaver/Golay/soft-RS toolkit the breadth modes (WS-B/C) assemble from.
pub mod conv;
pub mod fano;
pub mod fht;
pub mod interleave;
pub mod golay;
pub mod rs_gf64;

mod ft8_tables;
