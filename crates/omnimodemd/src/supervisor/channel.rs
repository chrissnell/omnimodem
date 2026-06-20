//! Channel configuration and runtime state.

use crate::ids::{ChannelId, DeviceId};
use crate::ptt::registry::PttConfig;

/// Persisted, operator-supplied channel configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelConfig {
    pub id: ChannelId,
    pub name: String,
    /// Mode label (e.g. "none"); the persisted form. Validated against and
    /// resolved to a parametric `crate::mode::ModeConfig` at configure time.
    pub mode: String,
    /// Audio device this channel binds to (the durable identity).
    pub device_id: DeviceId,
    /// Requested working rate (clamped to 48 kHz at open).
    pub sample_rate: u32,
    /// Capture fan-out consumers (0/1 == none).
    pub fanout: u32,
    /// PTT binding; `None` until ConfigurePtt.
    pub ptt: Option<PttConfig>,
}

/// Live channel state: its config plus whether the (stub) pipeline is running.
#[derive(Debug, Clone)]
pub struct ChannelState {
    pub config: ChannelConfig,
    pub running: bool,
}

impl ChannelState {
    pub fn new(config: ChannelConfig) -> Self {
        // No DSP in Phase 1, so a configured channel is immediately "running"
        // in the stub sense (it can accept simulated transmits).
        ChannelState { config, running: true }
    }
}
