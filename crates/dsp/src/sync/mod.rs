//! Group B — synchronization & acquisition building blocks.
//!
//! Clock recovery (DPLL), data-carrier detect (DCD), symbol-timing recovery,
//! Costas-loop carrier recovery + AFC, Costas-array generation/correlation,
//! sync-word/preamble matching, and STFT candidate finding. Every block is a
//! plain struct/function with inline self-consistent tests; nothing here
//! depends on other Group modules.

pub mod dpll;
pub mod dcd;
pub mod timing;
pub mod costas;
pub mod costas_array;
pub mod syncword;
pub mod candidate;
