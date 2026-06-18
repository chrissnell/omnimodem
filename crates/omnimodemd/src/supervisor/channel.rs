//! Channel configuration and runtime state.

use crate::ids::{ChannelId, DeviceId};

/// Persisted, operator-supplied channel configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelConfig {
    pub id: ChannelId,
    pub name: String,
    /// Phase 1 placeholder mode label (e.g. "none"); becomes a parametric
    /// `ModeConfig` in Phase 3.
    pub mode: String,
    /// Stable device this channel binds to (placeholder in Phase 1).
    pub device_id: DeviceId,
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
