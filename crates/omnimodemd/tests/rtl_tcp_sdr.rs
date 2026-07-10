//! Phase A · Plan 2 CLOSING GATE: an in-process fake `rtl_tcp` server streams an
//! FM-modulated AFSK1200 APRS burst as raw u8 IQ; the `RtlTcpBackend` connects,
//! reads the header, demodulates the IQ to audio, and the AFSK1200 ensemble
//! decodes the exact AX.25 frame back — end to end, no hardware.

use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
use omnimodem_dsp::mode::{Demodulator, Modulator};
use omnimodem_dsp::modes::afsk1200::{Afsk1200Demod, Afsk1200Mod};
use omnimodem_dsp::types::{Frame, FramePayload};
use omnimodemd::audio::backend::AudioBackend;
use omnimodemd::audio::rtlsdr::{RtlTcpBackend, DEFAULT_CAPTURE_RATE, DEFAULT_DEVIATION_HZ};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

const CHANNEL_RATE: u32 = 48_000;
const OFFSET_HZ: f32 = 30_000.0; // signal sits +30 kHz above the dongle center

/// A representative APRS position frame.
fn aprs_frame() -> Ax25Frame {
    Ax25Frame {
        dest: Address::new("APRS", 0),
        source: Address::new("K1ABC", 7),
        digipeaters: vec![],
        info: b"!4903.50N/07201.75W-RTL-SDR over rtl_tcp".to_vec(),
    }
}

/// Linearly upsample 48 kHz audio to the 240 kHz capture rate (integer 5:1).
fn upsample(audio: &[f32], factor: usize) -> Vec<f32> {
    if audio.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(audio.len() * factor);
    for i in 0..audio.len() * factor {
        let t = i as f32 / factor as f32;
        let base = t.floor() as usize;
        let frac = t - base as f32;
        let a = audio[base];
        let b = *audio.get(base + 1).unwrap_or(&a);
        out.push(a * (1.0 - frac) + b * frac);
    }
    out
}

/// FM-modulate `audio` (at `rate`) onto a carrier at `offset_hz`, `dev_hz` peak
/// deviation, and quantize to interleaved unsigned-8-bit IQ as `rtl_tcp` streams.
fn fm_modulate_u8(audio: &[f32], rate: f32, offset_hz: f32, dev_hz: f32) -> Vec<u8> {
    let mut phase = 0.0f32;
    let mut out = Vec::with_capacity(audio.len() * 2);
    for &a in audio {
        let inst = offset_hz + dev_hz * a;
        phase += std::f32::consts::TAU * inst / rate;
        let i = ((phase.cos() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
        let q = ((phase.sin() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
        out.push(i);
        out.push(q);
    }
    out
}

/// Start a fake `rtl_tcp` server: write the 12-byte header, drain client
/// commands, stream `iq`, then half-close so the client's read loop ends.
/// Returns the bound port.
fn spawn_fake_rtl_tcp(iq: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut header = [0u8; 12];
        header[0..4].copy_from_slice(b"RTL0");
        header[4..8].copy_from_slice(&5u32.to_be_bytes()); // R820T
        header[8..12].copy_from_slice(&29u32.to_be_bytes());
        sock.write_all(&header).unwrap();
        // Drain the tuning commands the client sends so its writes never block.
        let mut drain = sock.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut sink = [0u8; 64];
            while let Ok(n) = drain.read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
        });
        sock.write_all(&iq).unwrap();
        let _ = sock.shutdown(std::net::Shutdown::Write);
    });
    port
}

#[test]
fn fake_rtl_tcp_stream_decodes_aprs_frame() {
    let expected = aprs_frame();

    // Build the AFSK1200 audio for the frame, then FM-modulate it into IQ.
    let mut modulator = Afsk1200Mod::new();
    let audio = modulator.modulate(&Frame::packet(expected.encode())).unwrap();
    let up = upsample(&audio, (DEFAULT_CAPTURE_RATE / CHANNEL_RATE) as usize);
    let iq = fm_modulate_u8(&up, DEFAULT_CAPTURE_RATE as f32, OFFSET_HZ, DEFAULT_DEVIATION_HZ);

    let port = spawn_fake_rtl_tcp(iq);

    // Bind the backend, tune the NCO onto the +30 kHz signal, capture audio.
    let backend = RtlTcpBackend::new("127.0.0.1", port);
    backend.control().set_offset_hz(OFFSET_HZ);
    let cap = backend.open_capture(CHANNEL_RATE).unwrap();
    assert_eq!(cap.sample_rate, CHANNEL_RATE);

    // Drain the burst. The fake server serves one connection then closes; with the
    // Phase-D reconnect supervisor the capture keeps running (and retries) instead
    // of disconnecting, so collect until the stream goes quiet.
    let mut samples: Vec<f32> = Vec::new();
    loop {
        match cap.rx.recv_timeout(Duration::from_secs(2)) {
            Ok(chunk) => samples.extend(chunk.iter().map(|&s| s as f32 / 32768.0)),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(!samples.is_empty(), "no demodulated audio");

    // The AFSK1200 ensemble must recover the exact AX.25 frame with a good FCS.
    let mut demod = Afsk1200Demod::ensemble(9);
    let frames = demod.feed(&samples);
    let decoded = frames
        .iter()
        .find(|f| matches!(&f.payload, FramePayload::Packet(b) if *b == expected.encode()));
    let decoded = decoded.unwrap_or_else(|| {
        panic!(
            "no matching AX.25 frame decoded off the rtl_tcp stream (got {} frame(s))",
            frames.len()
        )
    });
    assert!(decoded.meta.crc_ok, "decoded frame failed FCS");
}
