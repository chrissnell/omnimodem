//! Strongly-typed identifiers used across the core/supervisor.

/// Logical channel id (matches the proto `channel` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(pub u32);

/// Per-process monotonic transmit id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransmitId(pub u64);

/// Stable device identity. In Phase 1 there is exactly one placeholder value;
/// Phase 2 replaces the inner string with a real cross-platform identity
/// derived from durable USB/ALSA/serial attributes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

impl DeviceId {
    /// The single placeholder device used until Phase 2 lands real detection.
    pub fn placeholder() -> Self {
        DeviceId("virtual:0".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_stable() {
        assert_eq!(DeviceId::placeholder(), DeviceId::placeholder());
        assert_eq!(DeviceId::placeholder().0, "virtual:0");
    }

    #[test]
    fn channel_and_transmit_ids_are_distinct_types() {
        let c = ChannelId(1);
        let t = TransmitId(1);
        assert_eq!(c.0 as u64, t.0); // values can match...
        // ...but the types cannot be confused at compile time (compile check).
    }
}
