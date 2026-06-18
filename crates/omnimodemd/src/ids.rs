//! Strongly-typed identifiers used across the core/supervisor.

/// Logical channel id (matches the proto `channel` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(pub u32);

/// Per-process monotonic transmit id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransmitId(pub u64);

/// Stable, cross-platform device identity.
///
/// Built from durable attributes, never a volatile `/dev` path or ALSA card
/// index, so config survives renames and hotplug. Ordered by preference: a USB
/// vendor/product/serial triple is the most durable; an ALSA stable card *name*
/// is next; USB port topology is the fallback for two identical adapters that
/// `by-id` cannot disambiguate; `Serial` wraps a `/dev/serial/by-id/<symlink>`
/// (already stable). `Placeholder` is retained for the file/stdin/loopback
/// backends and Phase-1 fixtures that have no physical identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeviceId {
    /// USB device by vendor/product + serial. `serial` is empty when the
    /// device exposes none (then prefer `Topology`).
    Usb { vid: u16, pid: u16, serial: String },
    /// ALSA sound card by its stable kernel *name* (e.g. "Device"), not index.
    AlsaCard { card_name: String },
    /// USB port topology: bus + port chain (e.g. "1-1.4.2"). Last resort for
    /// indistinguishable identical adapters.
    Topology { bus: u8, ports: String },
    /// A `/dev/serial/by-id/<id>` symlink target (durable by construction).
    Serial { by_id: String },
    /// Non-physical backend (file/stdin/loopback) or a Phase-1 fixture.
    Placeholder { tag: String },
}

impl DeviceId {
    /// The single placeholder identity used by virtual backends and fixtures.
    pub fn placeholder() -> Self {
        DeviceId::Placeholder { tag: "virtual:0".to_string() }
    }

    /// Canonical, round-trippable string form used as the persistence key and
    /// the gRPC `device_id` field. Format: `<scheme>:<body>`.
    pub fn to_canonical_string(&self) -> String {
        match self {
            DeviceId::Usb { vid, pid, serial } => {
                format!("usb:{vid:04x}:{pid:04x}:{serial}")
            }
            DeviceId::AlsaCard { card_name } => format!("alsa:{card_name}"),
            DeviceId::Topology { bus, ports } => format!("topo:{bus}-{ports}"),
            DeviceId::Serial { by_id } => format!("serial:{by_id}"),
            DeviceId::Placeholder { tag } => format!("virtual:{tag}"),
        }
    }

    /// Parse the canonical string form. `None` on an unrecognized scheme.
    pub fn parse(s: &str) -> Option<Self> {
        let (scheme, body) = s.split_once(':')?;
        match scheme {
            "usb" => {
                // usb:VVVV:PPPP:serial   (serial may be empty and may contain ':')
                let mut parts = body.splitn(3, ':');
                let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
                let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
                let serial = parts.next().unwrap_or("").to_string();
                Some(DeviceId::Usb { vid, pid, serial })
            }
            "alsa" => Some(DeviceId::AlsaCard { card_name: body.to_string() }),
            "topo" => {
                let (bus, ports) = body.split_once('-')?;
                Some(DeviceId::Topology { bus: bus.parse().ok()?, ports: ports.to_string() })
            }
            "serial" => Some(DeviceId::Serial { by_id: body.to_string() }),
            "virtual" => Some(DeviceId::Placeholder { tag: body.to_string() }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod device_id_tests {
    use super::*;

    fn roundtrip(id: DeviceId) {
        let s = id.to_canonical_string();
        assert_eq!(DeviceId::parse(&s), Some(id), "round-trip failed for {s}");
    }

    #[test]
    fn canonical_roundtrips_for_every_variant() {
        roundtrip(DeviceId::Usb { vid: 0x0d8c, pid: 0x013c, serial: "A1B2C3".into() });
        roundtrip(DeviceId::Usb { vid: 0x0d8c, pid: 0x013c, serial: "".into() });
        roundtrip(DeviceId::AlsaCard { card_name: "Device".into() });
        roundtrip(DeviceId::Topology { bus: 1, ports: "1.4.2".into() });
        roundtrip(DeviceId::Serial { by_id: "usb-FTDI_FT232R_AB0CDEFG-if00-port0".into() });
        roundtrip(DeviceId::placeholder());
    }

    #[test]
    fn usb_serial_may_contain_colons() {
        let id = DeviceId::Usb { vid: 1, pid: 2, serial: "a:b:c".into() };
        roundtrip(id);
    }

    #[test]
    fn placeholder_is_stable_and_canonical() {
        assert_eq!(DeviceId::placeholder(), DeviceId::placeholder());
        assert_eq!(DeviceId::placeholder().to_canonical_string(), "virtual:virtual:0");
    }

    #[test]
    fn unknown_scheme_is_none() {
        assert_eq!(DeviceId::parse("bogus:whatever"), None);
        assert_eq!(DeviceId::parse("noseparator"), None);
    }

    #[test]
    fn usb_is_preferred_over_topology_by_ord() {
        // Ord drives "most durable identity first" when ranking candidates.
        assert!(DeviceId::Usb { vid: 1, pid: 1, serial: "x".into() }
            < DeviceId::Topology { bus: 1, ports: "1".into() });
    }

    #[test]
    fn channel_and_transmit_ids_are_distinct_types() {
        let c = ChannelId(1);
        let t = TransmitId(1);
        assert_eq!(c.0 as u64, t.0); // values can match...
        // ...but the types cannot be confused at compile time (compile check).
    }
}
