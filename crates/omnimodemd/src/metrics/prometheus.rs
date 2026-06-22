//! Optional Prometheus exporter. Off by default; enabled by setting
//! `OMNIMODEM_PROMETHEUS_ADDR` (e.g. `127.0.0.1:9184`). Serves the standard
//! text-exposition format on `GET /metrics` over a tokio TCP listener — no extra
//! HTTP framework, just a tiny hand-rolled responder (the surface is one route).

use crate::metrics::ChannelMetricsSnapshot;

/// Render a set of per-channel snapshots to Prometheus text exposition.
pub fn render(snaps: &[ChannelMetricsSnapshot]) -> String {
    let mut s = String::new();
    s.push_str("# HELP omnimodem_good_frames Decoded frames with valid CRC.\n");
    s.push_str("# TYPE omnimodem_good_frames counter\n");
    s.push_str("# HELP omnimodem_bad_frames Decoded frames that failed CRC.\n");
    s.push_str("# TYPE omnimodem_bad_frames counter\n");
    s.push_str("# HELP omnimodem_snr_db Most recent decode SNR in dB.\n");
    s.push_str("# TYPE omnimodem_snr_db gauge\n");
    s.push_str("# HELP omnimodem_dbfs Most recent input level in dBFS.\n");
    s.push_str("# TYPE omnimodem_dbfs gauge\n");
    s.push_str("# HELP omnimodem_afc_offset_hz Most recent AFC frequency offset in Hz.\n");
    s.push_str("# TYPE omnimodem_afc_offset_hz gauge\n");
    s.push_str("# HELP omnimodem_dcd Data-carrier-detect state (1=detected).\n");
    s.push_str("# TYPE omnimodem_dcd gauge\n");
    for snap in snaps {
        let c = snap.channel.0;
        let m = &snap.metrics;
        s.push_str(&format!("omnimodem_good_frames{{channel=\"{c}\"}} {}\n", m.good_frames));
        s.push_str(&format!("omnimodem_bad_frames{{channel=\"{c}\"}} {}\n", m.bad_frames));
        s.push_str(&format!("omnimodem_snr_db{{channel=\"{c}\"}} {}\n", m.snr_db));
        s.push_str(&format!("omnimodem_dbfs{{channel=\"{c}\"}} {}\n", m.dbfs));
        s.push_str(&format!("omnimodem_afc_offset_hz{{channel=\"{c}\"}} {}\n", m.afc_offset_hz));
        s.push_str(&format!("omnimodem_dcd{{channel=\"{c}\"}} {}\n", u8::from(m.dcd)));
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
        let (mut sock, _) = listener.accept().await?;
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
    fn render_emits_one_block_per_channel() {
        let m = ChannelMetrics::default();
        let out = render(&[m.snapshot(ChannelId(0)), m.snapshot(ChannelId(1))]);
        assert!(out.contains("channel=\"0\""));
        assert!(out.contains("channel=\"1\""));
    }
}
