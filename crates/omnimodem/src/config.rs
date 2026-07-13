//! Daemon config file: an optional, line-oriented list of pre-registered
//! devices. It registers `rtl_tcp` SDR endpoints — network endpoints that no
//! hardware enumeration produces — so `ListDevices` can surface them for
//! selection, and it can pin a canonical `rtl:` id for a locally-attached
//! dongle (handy for labeling, or for a topology-keyed dongle you want present
//! in the list before it is plugged in). Ad-hoc binding of an unregistered
//! `rtltcp:host:port` still works without any config entry, and USB discovery
//! surfaces attached `rtl:` dongles automatically regardless of this file.
//!
//! Format — one directive per line; `#` starts a comment; blank lines ignored:
//!
//! ```text
//! # Shack SDR on the roof
//! rtl_tcp 192.168.1.50:1234 Rooftop R820T
//! rtl_tcp 127.0.0.1:1234
//! # A local dongle, labeled
//! rtl:serial:00000001 Attic dongle
//! ```
//!
//! A malformed line is skipped with a warning rather than failing daemon start,
//! so a typo never takes the daemon down.

use crate::device::DeviceDescriptor;
use crate::ids::DeviceId;
use std::path::Path;

/// Parse the daemon config text into the devices it registers. Unrecognized or
/// malformed lines are skipped (a `tracing::warn!` records each). The returned
/// descriptors are capture-only (`rtl_tcp` dongles are RX-only).
pub fn parse_registered_devices(text: &str) -> Vec<DeviceDescriptor> {
    let mut devices = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        // Strip an inline `#` comment, then trim.
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(desc) => devices.push(desc),
            None => tracing::warn!(line = lineno + 1, text = raw, "ignoring malformed config line"),
        }
    }
    devices
}

/// Parse one non-comment directive. Two forms:
/// * `rtl_tcp <host>:<port> [label...]` — a remote `rtl_tcp` endpoint.
/// * `rtl:<key> [label...]` — a canonical local RTL-SDR id (`rtl:serial:<s>` or
///   `rtl:topo:<bus>-<ports>`), parsed via [`DeviceId::parse`].
///
/// Both register a capture-only descriptor (RTL dongles are RX-only).
fn parse_line(line: &str) -> Option<DeviceDescriptor> {
    let mut parts = line.split_whitespace();
    let first = parts.next()?;
    let (id, default_label) = if first == "rtl_tcp" {
        let endpoint = parts.next()?;
        let id = DeviceId::parse(&format!("rtltcp:{endpoint}"))?;
        let (host, port) = match &id {
            DeviceId::RtlTcp { host, port } => (host.clone(), *port),
            _ => return None,
        };
        (id, format!("rtl_tcp {host}:{port}"))
    } else if first.starts_with("rtl:") {
        // The whole first token is the canonical id; the rest is the label.
        let id = DeviceId::parse(first)?;
        if !matches!(id, DeviceId::Rtl { .. }) {
            return None;
        }
        (id.clone(), id.to_canonical_string())
    } else {
        return None;
    };
    let label = parts.collect::<Vec<_>>().join(" ");
    let label = if label.is_empty() { default_label } else { label };
    // `needs_setup` is only known once the device is claimed; a config entry is
    // an assertion of identity, not of readiness.
    Some(DeviceDescriptor { id, label, has_capture: true, has_playback: false, needs_setup: false })
}

/// Load registered devices from `path`. A missing file is not an error — it just
/// yields no registered devices (the common case). An unreadable file logs a
/// warning and yields none.
pub fn load_registered_devices(path: &Path) -> Vec<DeviceDescriptor> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_registered_devices(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not read daemon config");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RtlKey;

    #[test]
    fn parses_local_rtl_ids_with_and_without_labels() {
        let cfg = "\
            rtl:serial:00000001 Attic dongle\n\
            rtl:topo:1-4.2\n";
        let devs = parse_registered_devices(cfg);
        assert_eq!(devs.len(), 2);

        assert_eq!(devs[0].id, DeviceId::Rtl { key: RtlKey::Serial("00000001".into()) });
        assert_eq!(devs[0].label, "Attic dongle");
        assert!(devs[0].has_capture && !devs[0].has_playback && !devs[0].needs_setup);

        // No label → the canonical id is used as the label.
        assert_eq!(
            devs[1].id,
            DeviceId::Rtl { key: RtlKey::Topo { bus: 1, ports: "4.2".into() } }
        );
        assert_eq!(devs[1].label, "rtl:topo:1-4.2");
    }

    #[test]
    fn rejects_malformed_and_non_local_rtl_ids() {
        // A remote `rtltcp:` id is not a local `rtl:` id; the bare token form is
        // reserved for local dongles, so this line is dropped.
        let devs = parse_registered_devices("rtltcp:host:1234\nrtl:bogus:x\n");
        assert!(devs.is_empty());
    }

    #[test]
    fn parses_endpoints_with_and_without_labels() {
        let cfg = "\
            # a comment\n\
            \n\
            rtl_tcp 192.168.1.50:1234 Rooftop R820T\n\
            rtl_tcp 127.0.0.1:1234\n";
        let devs = parse_registered_devices(cfg);
        assert_eq!(devs.len(), 2);

        assert_eq!(devs[0].id, DeviceId::RtlTcp { host: "192.168.1.50".into(), port: 1234 });
        assert_eq!(devs[0].label, "Rooftop R820T");
        assert!(devs[0].has_capture && !devs[0].has_playback);

        // No label → a synthesized one from the endpoint.
        assert_eq!(devs[1].id, DeviceId::RtlTcp { host: "127.0.0.1".into(), port: 1234 });
        assert_eq!(devs[1].label, "rtl_tcp 127.0.0.1:1234");
    }

    #[test]
    fn skips_comments_blanks_and_malformed_lines() {
        let cfg = "\
            rtl_tcp 10.0.0.1:2000\n\
            not_a_directive foo\n\
            rtl_tcp missing-port\n\
            rtl_tcp\n\
            rtl_tcp 10.0.0.2:3000 # trailing comment\n";
        let devs = parse_registered_devices(cfg);
        // Only the two well-formed rtl_tcp lines survive.
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].id, DeviceId::RtlTcp { host: "10.0.0.1".into(), port: 2000 });
        assert_eq!(devs[1].id, DeviceId::RtlTcp { host: "10.0.0.2".into(), port: 3000 });
        // The inline comment is stripped from the label.
        assert_eq!(devs[1].label, "rtl_tcp 10.0.0.2:3000");
    }

    #[test]
    fn hostname_endpoint_parses() {
        let devs = parse_registered_devices("rtl_tcp sdr.local:5678 Remote\n");
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].id, DeviceId::RtlTcp { host: "sdr.local".into(), port: 5678 });
    }

    #[test]
    fn missing_file_yields_no_devices() {
        let path = std::path::Path::new("/nonexistent/omnimodem/does-not-exist.conf");
        assert!(load_registered_devices(path).is_empty());
    }

    #[test]
    fn loads_from_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("omnimodem.conf");
        std::fs::write(&path, "rtl_tcp 192.168.0.9:1234 Attic\n").unwrap();
        let devs = load_registered_devices(&path);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].label, "Attic");
    }
}
