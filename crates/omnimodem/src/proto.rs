//! Generated gRPC types for the omnimodem.v1 package.

tonic::include_proto!("omnimodem.v1");

/// Proto package name, surfaced for handshake/debug.
pub const PACKAGE: &str = "omnimodem.v1";

/// Proto API major version. Within this major, changes are additive only.
pub const API_VERSION_MAJOR: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_types_are_constructible() {
        let req = ConfigureChannelRequest {
            channel: 0,
            name: "test".into(),
            mode: "none".into(),
            mode_params: None,
            rsid_tx: false,
            rsid_rx: false,
        };
        assert_eq!(req.name, "test");

        // The Event oneof must carry a snapshot variant.
        let ev = Event {
            kind: Some(event::Kind::Snapshot(ModemState { channels: vec![] })),
        };
        assert!(ev.kind.is_some());
    }

    #[test]
    fn phase2_types_are_constructible() {
        let _ = DeviceInfo {
            device_id: "usb:0d8c:013c:".into(),
            label: "C-Media".into(),
            has_capture: true,
            has_playback: true,
            needs_setup: false,
        };
        let _ = Event { kind: Some(event::Kind::PttState(PttState { channel: 0, keyed: true })) };
        assert_eq!(PttMethod::SerialRts as i32, 3);
    }
}
