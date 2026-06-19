//! Group A — front-end DSP & waveform building blocks.
//!
//! Front-end oscillators, FIR filters and filter design, NCO/down-converter,
//! polyphase resampler, STFT, AGC, hard limiter, FM/envelope detectors,
//! noise-floor/SNR reporting, and the symmetric modulator bank.
pub mod osc;
pub mod fir;
pub mod nco;
pub mod resample;
pub mod stft;
pub mod agc;
pub mod limiter;
pub mod detector;
pub mod noise;
pub mod modulate;
