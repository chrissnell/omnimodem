//! Group A — front-end DSP & waveform building blocks.
//!
//! Front-end oscillators, FIR filters and filter design, NCO/down-converter,
//! polyphase resampler, STFT, AGC, hard limiter, FM/envelope detectors,
//! noise-floor/SNR reporting, and the symmetric modulator bank.
//!
//! Deferred (Phase-5 follow-on, not yet present): the **OFDM core**
//! (`frontend::ofdm`) needed by the FreeDV / M17 / ARDOP voice-and-DV family.
//! That family is out of scope for the current Phase-5 plan (breadth +
//! integration & safety); its building-block group is named here so the
//! follow-on plan slots in cleanly.
pub mod osc;
pub mod fir;
pub mod nco;
pub mod resample;
pub mod spectrum;
pub mod stft;
pub mod agc;
pub mod limiter;
pub mod detector;
pub mod noise;
pub mod modulate;
