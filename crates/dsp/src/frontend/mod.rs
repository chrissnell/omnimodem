//! Group A — front-end DSP & waveform building blocks.
//!
//! Front-end oscillators, FIR filters and filter design, NCO/down-converter,
//! polyphase resampler, STFT, AGC, hard limiter, FM/envelope detectors,
//! noise-floor/SNR reporting, and the symmetric modulator bank.
//!
//! The **OFDM core** (`frontend::ofdm`) — 64-carrier overlapping-Walsh OFDM with
//! deep interleave — landed with Phase 11 (MT63) and is reused by Phase 16's OFDM
//! data modes.
pub mod osc;
pub mod fir;
pub mod msk;
pub mod multicarrier;
pub mod ofdm;
pub mod nco;
pub mod iq;
pub mod resample;
pub mod rsid;
pub mod spectrum;
pub mod stft;
pub mod complex_stft;
pub mod agc;
pub mod limiter;
pub mod detector;
pub mod squelch;
pub mod nbfm;
pub mod noise;
pub mod modulate;
