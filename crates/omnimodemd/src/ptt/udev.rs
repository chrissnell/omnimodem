//! udev rule suggestion. Produces a rule that creates a stable
//! /dev/omnimodem/<label> symlink keyed on the most durable attributes the
//! DeviceId carries. Never writes to disk.

use crate::ids::DeviceId;

/// Returns `(rule_text, instructions)` for the given identity. `None` for
/// identities a udev rule can't meaningfully pin (the virtual backends).
pub fn suggest(id: &DeviceId) -> Option<(String, String)> {
    let (matchers, label) = match id {
        DeviceId::Usb { vid, pid, serial } => {
            let mut m = format!(
                "ATTRS{{idVendor}}==\"{vid:04x}\", ATTRS{{idProduct}}==\"{pid:04x}\""
            );
            if !serial.is_empty() {
                m.push_str(&format!(", ATTRS{{serial}}==\"{serial}\""));
            }
            (m, format!("usb-{vid:04x}-{pid:04x}"))
        }
        DeviceId::Serial { by_id } => (
            format!("ENV{{ID_SERIAL}}==\"{by_id}\""),
            "serial".to_string(),
        ),
        DeviceId::Topology { bus, ports } => (
            format!("KERNELS==\"{bus}-{ports}\""),
            format!("topo-{bus}-{ports}"),
        ),
        DeviceId::AlsaCard { .. } | DeviceId::Placeholder { .. } => return None,
    };
    let rule = format!(
        "SUBSYSTEM==\"tty\", {matchers}, SYMLINK+=\"omnimodem/{label}\"\n"
    );
    let instructions = format!(
        "Save as /etc/udev/rules.d/70-omnimodem-{label}.rules, then run:\n  \
         sudo udevadm control --reload-rules && sudo udevadm trigger\n\
         The device will then appear at /dev/omnimodem/{label}."
    );
    Some((rule, instructions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_with_serial_emits_all_three_matchers() {
        let (rule, _) = suggest(&DeviceId::Usb {
            vid: 0x0d8c,
            pid: 0x013c,
            serial: "A1B2".into(),
        })
        .unwrap();
        assert!(rule.contains("idVendor}==\"0d8c\""));
        assert!(rule.contains("idProduct}==\"013c\""));
        assert!(rule.contains("serial}==\"A1B2\""));
        assert!(rule.contains("SYMLINK+=\"omnimodem/usb-0d8c-013c\""));
    }

    #[test]
    fn usb_without_serial_omits_serial_matcher() {
        let (rule, _) = suggest(&DeviceId::Usb { vid: 1, pid: 2, serial: "".into() }).unwrap();
        assert!(!rule.contains("serial}}"));
    }

    #[test]
    fn serial_by_id_uses_id_serial() {
        let (rule, _) = suggest(&DeviceId::Serial { by_id: "usb-FTDI_xyz".into() }).unwrap();
        assert!(rule.contains("ENV{ID_SERIAL}==\"usb-FTDI_xyz\""));
    }

    #[test]
    fn virtual_and_alsa_have_no_rule() {
        assert!(suggest(&DeviceId::placeholder()).is_none());
        assert!(suggest(&DeviceId::AlsaCard { card_name: "Device".into() }).is_none());
    }
}
