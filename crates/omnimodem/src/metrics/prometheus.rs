//! Optional Prometheus exporter. Off by default; enabled by setting
//! `OMNIMODEM_PROMETHEUS_ADDR` (e.g. `127.0.0.1:9184`). Serves the standard
//! text-exposition format on `GET /metrics` over a tokio TCP listener — no extra
//! HTTP framework, just a tiny hand-rolled responder (the surface is one route).

use crate::metrics::{ChannelMetrics, ChannelMetricsSnapshot};
use std::fmt::Write;

/// Format an f32 sample using Prometheus's special tokens for non-finite values
/// (`+Inf`/`-Inf`/`NaN`).
fn fmt_value(v: f32) -> String {
    if v.is_nan() {
        "NaN".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "+Inf".to_string() } else { "-Inf".to_string() }
    } else {
        v.to_string()
    }
}

/// One Prometheus metric family: (name, type, help, per-channel value renderer).
type Family = (&'static str, &'static str, &'static str, fn(&ChannelMetrics) -> String);

/// Render a set of per-channel snapshots to Prometheus text exposition. Each
/// metric family is emitted as one block (HELP + TYPE then all channels'
/// samples), so the output is valid even with multiple channels — a family's
/// samples must be contiguous.
pub fn render(snaps: &[ChannelMetricsSnapshot]) -> String {
    let mut s = String::new();
    let families: &[Family] = &[
        ("omnimodem_good_frames", "counter", "Decoded frames with valid CRC.", |m| {
            m.good_frames.to_string()
        }),
        ("omnimodem_bad_frames", "counter", "Decoded frames that failed CRC.", |m| {
            m.bad_frames.to_string()
        }),
        ("omnimodem_snr_db", "gauge", "Most recent decode SNR in dB.", |m| fmt_value(m.snr_db)),
        ("omnimodem_dbfs", "gauge", "Most recent input level in dBFS.", |m| fmt_value(m.dbfs)),
        ("omnimodem_afc_offset_hz", "gauge", "Most recent AFC frequency offset in Hz.", |m| {
            fmt_value(m.afc_offset_hz)
        }),
        ("omnimodem_dcd", "gauge", "Data-carrier-detect state (1=detected).", |m| {
            u8::from(m.dcd).to_string()
        }),
    ];
    for (name, typ, help, val) in families {
        let _ = writeln!(s, "# HELP {name} {help}");
        let _ = writeln!(s, "# TYPE {name} {typ}");
        for snap in snaps {
            let _ = writeln!(s, "{name}{{channel=\"{}\"}} {}", snap.channel.0, val(&snap.metrics));
        }
    }
    s
}

/// Serve `/metrics` until the task is dropped. `fetch` is called per scrape to
/// get the latest snapshots (the core's `GetMetrics` path).
pub async fn serve<F>(addr: std::net::SocketAddr, fetch: F) -> std::io::Result<()>
where
    F: Fn() -> Vec<ChannelMetricsSnapshot> + Send + Sync + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(addr).await?;
    loop {
        // A transient accept error must not kill the exporter permanently.
        let mut sock = match listener.accept().await {
            Ok((sock, _)) => sock,
            Err(e) => {
                tracing::warn!("prometheus accept error: {e}");
                continue;
            }
        };
        let body = render(&fetch());
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf).await; // drain the request line; single route
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ChannelId;
    use crate::metrics::ChannelMetrics;

    #[test]
    fn render_emits_labeled_series() {
        let m = ChannelMetrics { good_frames: 5, bad_frames: 1, snr_db: -7.5, ..Default::default() };
        let out = render(&[m.snapshot(ChannelId(2))]);
        assert!(out.contains("omnimodem_good_frames{channel=\"2\"} 5"));
        assert!(out.contains("omnimodem_bad_frames{channel=\"2\"} 1"));
        assert!(out.contains("omnimodem_snr_db{channel=\"2\"} -7.5"));
        assert!(out.contains("# TYPE omnimodem_good_frames counter"));
    }

    #[test]
    fn render_groups_samples_by_family_for_multiple_channels() {
        // Each metric family's samples must be contiguous (one HELP/TYPE then all
        // channels), or strict Prometheus parsers reject the exposition.
        let m = ChannelMetrics::default();
        let out = render(&[m.snapshot(ChannelId(0)), m.snapshot(ChannelId(1))]);
        let lines: Vec<&str> = out.lines().collect();
        let good: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("omnimodem_good_frames{"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(good.len(), 2, "two channels");
        assert_eq!(good[1], good[0] + 1, "good_frames samples must be contiguous");
        // Exactly one HELP line per family.
        assert_eq!(out.matches("# HELP omnimodem_good_frames").count(), 1);
    }

    #[test]
    fn render_uses_prometheus_nonfinite_tokens() {
        let m = ChannelMetrics { snr_db: f32::NEG_INFINITY, dbfs: f32::NAN, ..Default::default() };
        let out = render(&[m.snapshot(ChannelId(0))]);
        assert!(out.contains("omnimodem_snr_db{channel=\"0\"} -Inf"));
        assert!(out.contains("omnimodem_dbfs{channel=\"0\"} NaN"));
    }
}
