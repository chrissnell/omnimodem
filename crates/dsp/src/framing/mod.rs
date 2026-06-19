//! Group D — source, message and framing coding.
//!
//! Each block documents and asserts its on-wire bit order. AX.25/HDLC and the
//! amateur teletype/CW codes are **LSB-first on the wire**; the WSJT-X 77-bit
//! message is **MSB-first big-endian** into the LDPC. A round-trip test that
//! does not assert bit order is treated as incomplete (see "Conventions
//! locked here" in the Phase-3 plan).
pub mod varicode;
pub mod baudot;
pub mod morse;
pub mod hdlc;
pub mod ax25;
pub mod fx25;
pub mod il2p;
pub mod message77;
