//! Mode assemblies: each file wires Phase-3 building blocks (`frontend`,
//! `sync`, `fec`, `framing`, `ensemble`) into a concrete `Demodulator` /
//! `BlockDemodulator` and a symmetric `Modulator` for one end-user mode.
//!
//! These are pure DSP — no daemon, no audio device — so every mode loopback-,
//! KAT-, and BER-tests in CI. The daemon's `mode::registry` maps a parametric
//! `ModeConfig` onto these constructors; nothing else learns mode specifics.

pub mod afsk1200;
pub mod contestia;
pub mod cw;
pub mod dominoex;
pub mod hell;
pub mod ft8;
pub mod fsk_util;
pub mod fsq;
pub mod fsq_pic;
pub mod ft4;
pub mod fst4;
pub mod ifk33;
pub mod ifkp;
pub mod msk144;
pub mod ifkp_pic;
pub mod jt4;
pub mod jt65;
pub mod jt9;
pub mod mfsk;
pub mod mfsk_pic;
pub mod mt63;
pub mod navtex;
pub mod olivia;
pub mod picture;
pub mod psk;
pub mod psk31;
pub mod rtty;
pub mod thor;
pub mod thor_pic;
pub mod throb;
pub mod wefax;
pub mod wspr;
