//! Group D — source, message and framing coding.
//!
//! Each block documents and asserts its on-wire bit order. AX.25/HDLC and the
//! amateur teletype/CW codes are **LSB-first on the wire**; the WSJT-X 77-bit
//! message is **MSB-first big-endian** into the LDPC. A round-trip test that
//! does not assert bit order is treated as incomplete (see "Conventions
//! locked here" in the Phase-3 plan).
//!
//! Deferred (Phase-5 follow-on, not yet present): the **vocoder interface**
//! (`framing::vocoder`) and the **ARQ engine** required by the FreeDV / M17 /
//! ARDOP family and the (external) KISS↔gRPC translator. Those are out of scope
//! for the current Phase-5 plan; their groups are named here to keep the catalog
//! map honest.
pub mod varicode;
pub mod jsc;
pub mod js8_message;
pub mod js8_callsign;
pub mod dominoex_varicode;
pub mod thor_varicode;
pub mod ifkp_varicode;
pub mod fsq_varicode;
pub mod hellfont;
pub mod baudot;
pub mod morse;
pub mod hdlc;
pub mod ax25;
pub mod fx25;
pub mod il2p;
pub mod message77;
pub mod pack77;
