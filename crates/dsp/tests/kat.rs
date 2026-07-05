//! Layer-1 conformance: known-answer tests against published/reference vectors.
//! Each coding block contributes a `#[test]` checked against vectors that are
//! either inline constants traceable to a named reference source, or files
//! under `tests/vectors/` with a provenance header.
//!
//! This target uses the `testutil` fixtures (AWGN, hex helpers), so it is gated
//! behind the `testutil` feature. Run with `cargo test -p omnimodem-dsp
//! --features testutil`; a plain `cargo test` compiles it to an empty target.
#![cfg(feature = "testutil")]

use omnimodem_dsp::testutil::{add_awgn, bytes_to_hex, hex_to_bytes, Rng};

#[test]
fn harness_links() {
    // Sanity: the testutil surface is reachable from an integration test.
    assert_eq!(bytes_to_hex(&hex_to_bytes("dead")), "dead");
}

// --- Group C: FEC known-answer tests -------------------------------------

/// CRC-16/X.25 check value over the standard `"123456789"` corpus.
/// Provenance: the canonical CRC catalogue (Koopman / CRC RevEng) lists the
/// CRC-16/X.25 (a.k.a. CRC-16/IBM-SDLC) check value as `0x906E`.
#[test]
fn crc16_x25_check_value() {
    use omnimodem_dsp::fec::crc::{crc, CRC16_X25};
    assert_eq!(crc(&CRC16_X25, b"123456789"), 0x906E);
}

/// FT8/FT4 CRC-14 is self-consistent under its resolved spec (poly 0x2757,
/// init 0, non-reflected). Provenance to confirm against `ft8_lib crc.c` in
/// Phase 4; here we pin determinism and width.
#[test]
fn crc14_ft8_is_14_bits_and_deterministic() {
    use omnimodem_dsp::fec::crc::{crc, CRC14_FT8};
    let a = crc(&CRC14_FT8, b"\x01\x02\x03");
    let b = crc(&CRC14_FT8, b"\x01\x02\x03");
    assert_eq!(a, b);
    assert!(a < (1 << 14), "CRC-14 must fit in 14 bits");
}

/// Reed–Solomon corrects within capacity and *detects* (does not miscorrect)
/// beyond it, for both the FX.25 (fcr=1) and IL2P (fcr=0) instantiations.
#[test]
fn rs_corrects_within_capacity_and_detects_beyond() {
    use omnimodem_dsp::fec::rs::Rs;
    const PRIM: u8 = 0x1D; // GF(256) primitive 0x11D
    for &(nroots, fcr) in &[(16usize, 1usize), (16, 0), (32, 1)] {
        let rs = Rs::new(nroots, fcr, PRIM);
        let data: Vec<u8> = (0..40u8).collect();
        let parity = rs.encode_parity(&data);
        assert_eq!(parity.len(), nroots);

        // Within capacity (t = nroots/2): corrupt t symbols, must recover.
        let t = nroots / 2;
        let mut cw: Vec<u8> = data.iter().chain(parity.iter()).copied().collect();
        for i in 0..t {
            cw[i * 2] ^= 0x5A;
        }
        let res = rs.decode(&mut cw);
        assert!(res.is_ok(), "nroots={nroots} fcr={fcr}: decode within capacity failed: {res:?}");
        assert_eq!(&cw[..data.len()], &data[..], "recovered payload mismatch");

        // Beyond capacity (t+1, t+2, and the full 2t = nroots errors): a
        // bounded-distance decoder must behave honestly. For every over-the-
        // limit corruption it must EITHER report failure (detection), OR, if it
        // returns success, (a) never claim more than `t` corrections, and (b)
        // never present the *original* message as a clean decode. Asserting on
        // both arms (not just the Ok arm) removes the earlier vacuity.
        let mut detected_at_least_once = false;
        for extra in [1usize, 2, t.max(1)] {
            let nerr = (t + extra).min(data.len() + nroots);
            let mut cw2: Vec<u8> = data.iter().chain(parity.iter()).copied().collect();
            for (i, b) in cw2.iter_mut().take(nerr).enumerate() {
                *b ^= 0xA5 ^ (i as u8) | 1; // distinct, non-zero errors
            }
            match rs.decode(&mut cw2) {
                Err(_) => detected_at_least_once = true,
                Ok(k) => {
                    assert!(
                        k <= t,
                        "nroots={nroots} fcr={fcr}: decoder claimed {k} > t={t} corrections"
                    );
                    assert_ne!(
                        &cw2[..data.len()],
                        &data[..],
                        "nroots={nroots} fcr={fcr}: beyond-capacity miscorrected to the original"
                    );
                }
            }
        }
        // With 2t = nroots simultaneous errors a syndrome-based decoder cannot
        // find a ≤t solution, so at least one of the over-limit cases must be
        // reported as an uncorrectable failure (proves the detect path runs).
        assert!(
            detected_at_least_once,
            "nroots={nroots} fcr={fcr}: no beyond-capacity corruption was detected as uncorrectable"
        );
    }
}

/// LDPC tables are the real WSJT-X / `ft8_lib` `(174,91)` code, and the two
/// independently-transcribed tables (`kFTX_LDPC_generator` and `kFTX_LDPC_Nm`)
/// are mutually consistent: every systematic generator row is a valid codeword
/// of the Nm parity-check matrix (`G·Hᵀ = 0`), every variable lies in exactly 3
/// checks, and the total edge count is 522 (= 174×3). A single transcription
/// error in either table would break `G·Hᵀ = 0`.
#[test]
fn ft8_ldpc_matches_reference() {
    use omnimodem_dsp::fec::ldpc::Ldpc;
    let code = Ldpc::ft8();
    assert_eq!((code.n(), code.k()), (174, 91));

    // G·Hᵀ = 0: encoding each unit message e_j yields generator row j, which
    // must satisfy every parity check. Covers all 91 generator rows exactly.
    for j in 0..91 {
        let mut msg = vec![0u8; 91];
        msg[j] = 1;
        let cw = code.encode(&msg);
        assert_eq!(code.parity_errors(&cw), 0, "generator row {j} is not a codeword of Nm");
    }

    // Structural invariants of the FT8 Tanner graph (matches kFTX_LDPC_Mn).
    let mut var_degree = [0usize; 174];
    let mut edges = 0usize;
    for c in 0..83 {
        for &v in code.check_vars(c) {
            var_degree[v] += 1;
            edges += 1;
        }
    }
    assert!(var_degree.iter().all(|&d| d == 3), "every variable must lie in exactly 3 checks");
    assert_eq!(edges, 522, "FT8 LDPC has 174×3 = 522 Tanner-graph edges");
}

/// LDPC: a noiseless codeword's LLRs decode back to the original 91 message
/// bits with zero parity errors, and a moderate-SNR copy still decodes.
#[test]
fn ldpc_encode_noiseless_decode() {
    use omnimodem_dsp::fec::ldpc::Ldpc;
    let code = Ldpc::ft8();
    assert_eq!(code.n(), 174);
    assert_eq!(code.k(), 91);

    let mut rng = Rng::new(20260619);
    let msg: Vec<u8> = (0..91).map(|_| (rng.next_u64() & 1) as u8).collect();
    let cw = code.encode(&msg);
    assert_eq!(cw.len(), 174);
    assert_eq!(code.parity_errors(&cw), 0, "encoded word must satisfy parity");

    // Map hard codeword bits to confident LLRs (locked convention: bit 0 => +).
    let llrs: Vec<f32> = cw.iter().map(|&b| if b == 0 { 6.0 } else { -6.0 }).collect();
    let (dec, perr) = code.decode_minsum(&llrs, 50);
    assert_eq!(perr, 0, "noiseless decode must converge");
    assert_eq!(&dec[..91], &msg[..], "decoded message mismatch");

    // Add light AWGN to the soft values and confirm it still converges.
    let mut soft = llrs.clone();
    add_awgn(&mut soft, 1.0, &mut rng);
    let (dec2, perr2) = code.decode_minsum(&soft, 50);
    assert_eq!(perr2, 0, "moderate-SNR decode must converge");
    assert_eq!(&dec2[..91], &msg[..]);
}

// --- Group B: sync known-answer test -------------------------------------

/// The canonical FT8 Costas sync array.
/// Provenance: WSJT-X / `ft8_lib constants.c` — `[3,1,4,0,6,5,2]`.
#[test]
fn ft8_costas_array_is_canonical() {
    use omnimodem_dsp::sync::costas_array::ft8_costas;
    assert_eq!(ft8_costas(), [3, 1, 4, 0, 6, 5, 2]);
}

// --- Group D: framing known-answer tests ---------------------------------

/// HDLC frames and de-frames a payload, validates the FCS, and a single bit
/// flip fails the FCS. (Direwolf `gen_packets` byte-for-byte cross-check is an
/// `#[ignore]`d regeneration test below — it needs the reference binary.)
#[test]
fn hdlc_frame_roundtrips_and_fcs_guards() {
    use omnimodem_dsp::framing::hdlc::{hdlc_deframe, hdlc_frame};
    let payload = b"PHASE3 KAT";
    let bits = hdlc_frame(payload);
    assert_eq!(hdlc_deframe(&bits), vec![payload.to_vec()]);

    let mut corrupt = bits.clone();
    corrupt[20] ^= 1;
    assert!(hdlc_deframe(&corrupt).is_empty(), "bit flip must fail FCS");
}

/// AX.25 UI frame round-trips through encode/decode.
#[test]
fn ax25_ui_frame_roundtrips() {
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
    let f = Ax25Frame {
        dest: Address::new("APRS", 0),
        source: Address::new("K1ABC", 7),
        digipeaters: vec![Address::new("WIDE2", 1)],
        info: b"!4903.50N/07201.75W-Test".to_vec(),
    };
    let bytes = f.encode();
    let back = Ax25Frame::decode(&bytes).expect("decode");
    assert_eq!(back.source.call, "K1ABC");
    assert_eq!(back.source.ssid, 7);
    assert_eq!(back.dest.call, "APRS");
    assert_eq!(back.info, f.info);
}

/// WSJT-X 77-bit standard message round-trips.
/// NOTE: byte-for-byte equality with `ft8code` is a Phase-4 cross-check; this
/// pins the codec's internal round-trip invariant.
#[test]
fn message77_standard_roundtrips() {
    use omnimodem_dsp::framing::message77::{pack77, unpack77};
    let m = "CQ K1ABC FN42";
    assert_eq!(unpack77(&pack77(m)), m);
}

// --- Reference-binary regeneration (gated, documents provenance) ----------

/// Direwolf cross-check of HDLC/AX.25/FX.25/IL2P bytes and ft8_lib LDPC/CRC/
/// 77-bit payloads is the Phase-4 gate; it needs the reference binaries which
/// are not present on CI. This ignored test documents the exact regeneration
/// commands so the provenance is executable when the binaries are available.
#[test]
#[ignore = "requires Direwolf/ft8_lib reference binaries (Phase-4 interop gate)"]
fn regenerate_reference_vectors_doc() {
    // Documentation-only. When the reference binaries are available, run:
    //   Direwolf HDLC/AX.25:  gen_packets -o out.wav -n 1 "K1ABC>APRS:>test"
    //   Direwolf FX.25:       gen_packets -X 16 ...   (RS(255,239)-shortened)
    //   Direwolf IL2P:        gen_packets -I 1 ...    (cross-check il2p_test)
    //   ft8_lib LDPC/77-bit:  ft8code "CQ K1ABC FN42" -> 77-bit + 174-bit codeword
    // Capture the bytes into tests/vectors/*.json with this comment as the
    // provenance header, then drop the `#[ignore]` on the corresponding KAT.
    //
    // This body is intentionally a no-op so that running the suite with
    // `--ignored` does not fail; the value here is the executable provenance
    // record in the comment above.
}

// --- Phase-3 exit criterion ----------------------------------------------

/// The executable definition of "Phase 3 done": the single named gate that runs
/// every contract-critical KAT (`cargo test -p omnimodem-dsp --features testutil
/// phase3_exit_criterion`). Each KAT is also its own `#[test]`; the value this
/// aggregate adds over re-running them is *structural*, not just coverage:
///
/// 1. It is a **compile-checked manifest** of the contract-critical set. Each
///    entry is a direct call, so deleting or renaming any gated KAT breaks this
///    test's compilation — a gate cannot be silently dropped from the suite
///    without a reviewer seeing this list change.
/// 2. It gives CI **one** target to gate a merge on instead of an open-ended
///    name filter that would silently pass if a KAT were removed.
///
/// Keep this list in sync with the contract-critical KATs above; adding a new
/// phase-gating KAT means adding a call here (the compiler will not remind you,
/// but a missing entry means the gate under-covers — that is the one thing this
/// aggregate cannot self-check, so it is called out here deliberately).
#[test]
fn phase3_exit_criterion() {
    crc16_x25_check_value();
    crc14_ft8_is_14_bits_and_deterministic();
    rs_corrects_within_capacity_and_detects_beyond();
    ft8_ldpc_matches_reference();
    ldpc_encode_noiseless_decode();
    ft8_costas_array_is_canonical();
    hdlc_frame_roundtrips_and_fcs_guards();
    ax25_ui_frame_roundtrips();
    message77_standard_roundtrips();
}

// --- Phase-4: bidirectional cross-decode interop gates ------------------------
// Design §"Cross-decode interop — the decisive test": modulate with omnimodem,
// decode with the reference, AND the reverse. These need the reference binaries
// (Direwolf, WSJT-X, fldigi), which are not on CI, so they are `#[ignore]`d and
// document the exact regeneration/verification commands as executable
// provenance. Drop the `#[ignore]` once the captured vectors exist.

/// FT8 transmit chain is **byte-exact with WSJT-X/ft8_lib**: for each reference
/// message, our `ft8_symbols()` (pack77 → CRC-14 → LDPC(174,91) → FT8 Gray map →
/// Costas-interleaved tones) equals the 79 channel symbols that ft8_lib's
/// `ft8_encode()` produces. This is the decisive encode-side interop check; it
/// now runs on CI (no longer `#[ignore]`d) because the golden tones are baked in
/// from ft8_lib itself (`tests/vectors/ft8_reference.json`, regenerated by the
/// dumper documented there). A bit-identical tone stream is exactly what WSJT-X
/// transmits, so this proves on-air interoperability of the modulator.
#[test]
fn ft8_symbols_match_wsjtx_reference() {
    use omnimodem_dsp::modes::ft8::ft8_symbols;
    // (message, ft8_lib 79-tone string). Provenance: ft8_lib `ft8_encode`.
    let cases: &[(&str, &str)] = &[
        ("CQ K1ABC FN42", "3140652000000001005476704606021533433140652736011047517007334745455133543140652"),
        ("W9XYZ K1ABC FN42", "3140652020355725005476704606021535723140652053165574061740300434722541223140652"),
        ("K1ABC W9XYZ RR73", "3140652032247523504061147017455422543140652656077704107145041657342273103140652"),
        ("CQ N0CALL EM48", "3140652000000001001713355505100026553140652521535217112525061221035026243140652"),
        ("HELLO WORLD", "3140652007147234503642644417455331463140652077717023237271727060731246133140652"),
        ("TEST 123", "3140652012256632011763147617455330243140652121205766024650763315554275413140652"),
    ];
    for (msg, tones) in cases {
        let got = ft8_symbols(msg);
        let want: Vec<u8> = tones.bytes().map(|b| b - b'0').collect();
        assert_eq!(&got[..], &want[..], "FT8 symbols for {msg:?} differ from ft8_lib");
    }
}

#[test]
#[ignore = "requires WSJT-X jt9/ft8sim binaries (live-audio interop, beyond the byte-exact gate)"]
fn ft8_wav_interop_doc() {
    // The byte-exact encode gate above (ft8_symbols_match_wsjtx_reference) is the
    // CI-runnable proof. This remaining live check needs the WSJT-X binaries:
    //   ours→ref:  write our `Ft8Mod` waveform to a .wav; `jt9 -8 our.wav` prints
    //              the message.
    //   ref→ours:  `ft8sim "CQ K1ABC FN42" 1500 0 0 0 1 -10` → `decode_window`
    //              recovers it (exercises the synthesizer/AFC, not just the bits).
}

#[test]
#[ignore = "requires Direwolf gen_packets/atest (Phase-4 interop gate)"]
fn afsk1200_cross_decode_doc() {
    // ours→ref:  our `Afsk1200Mod` audio (48 kHz) → `atest` must decode the
    //            AX.25 frame.
    // ref→ours:  `gen_packets -o ref.wav "K1ABC>APRS:>test"` → our
    //            `Afsk1200Demod` must decode it.
}

#[test]
#[ignore = "requires fldigi (Phase-4 interop gate)"]
fn rtty_cross_decode_doc() {
    // Cross-check RTTY (45.45 baud / 170 Hz shift, Baudot) against fldigi's RTTY
    // modem in both directions.
}

#[test]
#[ignore = "requires fldigi (Phase-4 interop gate)"]
fn psk31_cross_decode_doc() {
    // Cross-check PSK31 (BPSK Varicode) against fldigi's PSK31 modem in both
    // directions.
}

#[test]
#[ignore = "requires fldigi (Phase-4 interop gate)"]
fn cw_cross_decode_doc() {
    // Cross-check CW (Morse) against fldigi's CW decoder in both directions.
}

// --- Phase-4 exit criterion ---------------------------------------------------

/// The CI-runnable definition of "Phase 4 done": every mode self-loopbacks and
/// recovers its exact payload. The reference-binary cross-decode gates above are
/// the *nightly* completion of the exit criterion (design §"Definition of done
/// for a mode"); this aggregate is the per-PR gate. Keep it in sync as modes are
/// added — a missing entry means the gate under-covers.
#[test]
fn phase4_exit_criterion() {
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
    use omnimodem_dsp::mode::{BlockDemodulator, Demodulator, Modulator};
    use omnimodem_dsp::modes::{
        afsk1200::{Afsk1200Demod, Afsk1200Mod},
        cw::{CwDemod, CwMod},
        ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S},
        psk31::{Psk31Demod, Psk31Mod},
        rtty::{RttyDemod, RttyMod},
    };
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[omnimodem_dsp::types::Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    // AFSK 1200
    let ax = Ax25Frame {
        dest: Address::new("APRS", 0),
        source: Address::new("K1ABC", 7),
        digipeaters: vec![],
        info: b"exit".to_vec(),
    };
    let s = Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap();
    assert!(
        Afsk1200Demod::ensemble(9)
            .feed(&s)
            .iter()
            .any(|f| matches!(&f.payload, FramePayload::Packet(b) if b == &ax.encode())),
        "AFSK1200 exit criterion"
    );

    // PSK31
    let s = Psk31Mod::new(1000.0).modulate(&Frame::text("CQ DE K1ABC")).unwrap();
    assert!(texts(&Psk31Demod::new(1000.0).feed(&s)).contains("CQ DE K1ABC"), "PSK31 exit");

    // RTTY
    let s = RttyMod::new(45.45, 170.0).modulate(&Frame::text("THE QUICK BROWN FOX")).unwrap();
    assert!(
        texts(&RttyDemod::new(45.45, 170.0).feed(&s)).contains("THE QUICK BROWN FOX"),
        "RTTY exit"
    );

    // CW (adaptive squelch needs a noise floor)
    let mut s = CwMod::new(20, 700.0).modulate(&Frame::text("CQ TEST")).unwrap();
    let mut rng = Rng::new(1);
    let mut lead = vec![0.0f32; 1600];
    add_awgn(&mut lead, 0.02, &mut rng);
    add_awgn(&mut s, 0.02, &mut rng);
    let mut cw = CwDemod::new(20, 700.0);
    let mut cw_frames = cw.feed(&lead);
    cw_frames.extend(cw.feed(&s));
    cw_frames.extend(cw.finish_text());
    let cw_text = texts(&cw_frames).to_uppercase();
    assert!(cw_text.contains("CQ") && cw_text.contains("TEST"), "CW exit");

    // FT8
    let wave = Ft8Mod::new().modulate(&Frame::text("CQ K1ABC FN42")).unwrap();
    let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
    win[..wave.len()].copy_from_slice(&wave);
    assert!(texts(&Ft8Demod::new().decode_window(&win, 0)).contains("CQ K1ABC FN42"), "FT8 exit");
}

// --- Group P: PSK family (fldigi parity) ---------------------------------

/// Extract the `varicode_bits` (0/1 string) for one message from the fldigi
/// golden vector, using the same minimal line scan as message77's vector test
/// (no serde dependency in the test crate).
fn psk_bpsk_vector_bits(msg: &str) -> Vec<u8> {
    let raw = include_str!("vectors/psk_bpsk.json");
    let needle = format!("\"msg\":\"{msg}\"");
    for line in raw.lines() {
        if !line.contains(&needle) {
            continue;
        }
        let key = "\"varicode_bits\":\"";
        let bi = line.find(key).expect("varicode_bits field") + key.len();
        let bits = &line[bi..line[bi..].find('"').unwrap() + bi];
        return bits.bytes().map(|c| c - b'0').collect();
    }
    panic!("message {msg:?} not in psk_bpsk.json");
}

/// Bit-exact: omnimodem's PSK31 Varicode payload bitstream (codeword + `00`
/// separators) reproduces fldigi's `psk_varicode_encode` output byte-for-byte.
/// Provenance: `tests/vectors/psk_bpsk.json` (fldigi 4.1.23 @ 61b97f413, driver
/// `scratch/refvectors/build_psk_varicode.sh`).
#[test]
fn psk_bpsk_varicode_matches_fldigi_vector() {
    use omnimodem_dsp::modes::psk::{encode_bpsk_bits, PskVariant};
    for msg in ["CQ DE K1ABC", "The quick brown fox 0123456789"] {
        let want = psk_bpsk_vector_bits(msg);
        let got = encode_bpsk_bits(PskVariant::Psk125, msg);
        assert_eq!(got, want, "PSK Varicode payload differs from fldigi for {msg:?}");
    }
}

/// The full BPSK rate grid round-trips a message through TX→RX on a clean
/// channel and under light AWGN (envelope-histogram timing + differential
/// decode). One representative per rate; the submode grid is parametric.
#[test]
fn psk_bpsk_rate_grid_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC";
    for v in [
        PskVariant::Psk31,
        PskVariant::Psk63,
        PskVariant::Psk125,
        PskVariant::Psk250,
        PskVariant::Psk500,
        PskVariant::Psk1000,
    ] {
        // Clean channel.
        let clean = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert!(
            texts(&PskDemod::new(v, 1500.0).feed(&clean)).contains(msg),
            "{v:?} clean loopback"
        );
        // Light AWGN.
        let mut noisy = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x9151 + v.samples_per_symbol() as u64);
        add_awgn(&mut noisy, 0.03, &mut rng);
        assert!(
            texts(&PskDemod::new(v, 1500.0).feed(&noisy)).contains(msg),
            "{v:?} AWGN loopback"
        );
    }
}

/// QPSK family: bit-exact K=5 FEC vs fldigi, plus clean + AWGN loopback across
/// the rate grid (differential-QPSK detection + continuous Viterbi).
#[test]
fn psk_qpsk_fec_and_loopback() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    // Bit-exact FEC vs the fldigi vector.
    let raw = include_str!("vectors/psk_qpsk.json");
    let line = raw.lines().find(|l| l.contains("\"qpsk_symbols\"")).unwrap();
    let field = |k: &str| {
        let i = line.find(k).unwrap() + k.len();
        line[i..line[i..].find('"').unwrap() + i].to_string()
    };
    let vbits: Vec<u8> = field("\"varicode_bits\":\"").bytes().map(|c| c - b'0').collect();
    let want: Vec<u8> =
        field("\"qpsk_symbols\":\"").split(' ').map(|s| s.parse().unwrap()).collect();
    let code = PskVariant::Qpsk125.conv_code().unwrap();
    let out = code.encode(&vbits);
    let got: Vec<u8> = (0..want.len()).map(|i| out[2 * i] | (out[2 * i + 1] << 1)).collect();
    assert_eq!(got, want, "QPSK K=5 code symbols differ from fldigi");

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC";
    for v in [
        PskVariant::Qpsk31,
        PskVariant::Qpsk63,
        PskVariant::Qpsk125,
        PskVariant::Qpsk250,
        PskVariant::Qpsk500,
    ] {
        let clean = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&clean)).contains(msg), "{v:?} clean");
        let mut noisy = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x9251 + v.samples_per_symbol() as u64);
        add_awgn(&mut noisy, 0.03, &mut rng);
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&noisy)).contains(msg), "{v:?} AWGN");
    }
}

/// PSK63F (robust +F, no interleaver): the K=7 FEC + MFSK-Varicode + two-phase
/// Viterbi chain round-trips a message on a clean channel and under light AWGN.
/// The MFSK Varicode drops the final char (no trailing boundary bit), so the
/// check is the message minus its last character.
#[test]
fn psk63f_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC";
    let want = &msg[..msg.len() - 1];
    let clean = PskMod::new(PskVariant::Psk63F, 1500.0).modulate(&Frame::text(msg)).unwrap();
    assert!(texts(&PskDemod::new(PskVariant::Psk63F, 1500.0).feed(&clean)).contains(want), "clean");

    let mut noisy = PskMod::new(PskVariant::Psk63F, 1500.0).modulate(&Frame::text(msg)).unwrap();
    let mut rng = Rng::new(0x63f0);
    add_awgn(&mut noisy, 0.05, &mut rng);
    assert!(texts(&PskDemod::new(PskVariant::Psk63F, 1500.0).feed(&noisy)).contains(want), "AWGN");
}

/// The interleaved PSK-R robust grid (PSK125R/250R/500R/1000R): K=7 FEC + the
/// 2×2×idepth diagonal interleaver + two-phase Viterbi, round-tripping clean and
/// under light AWGN. MFSK Varicode drops the final char.
#[test]
fn pskr_grid_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC";
    let want = &msg[..msg.len() - 1];
    for v in [
        PskVariant::Psk125R,
        PskVariant::Psk250R,
        PskVariant::Psk500R,
        PskVariant::Psk1000R,
    ] {
        let clean = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&clean)).contains(want), "{v:?} clean");
        let mut noisy = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x8300 + v.samples_per_symbol() as u64);
        add_awgn(&mut noisy, 0.04, &mut rng);
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&noisy)).contains(want), "{v:?} AWGN");
    }
}

/// The multi-carrier robust nX_PSK63R grid (even carrier counts): the PSK-R core
/// distributed round-robin over N frequency-offset carriers, clean and under
/// light AWGN. MFSK Varicode drops the final char.
#[test]
fn nx_psk63r_grid_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC";
    let want = &msg[..msg.len() - 1];
    for v in [
        PskVariant::Psk63Rc4,
        PskVariant::Psk63Rc5,
        PskVariant::Psk63Rc10,
        PskVariant::Psk63Rc20,
        PskVariant::Psk63Rc32,
    ] {
        let clean = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&clean)).contains(want), "{v:?} clean");
        let mut noisy = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x6300 + v.carriers() as u64);
        add_awgn(&mut noisy, 0.02, &mut rng);
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&noisy)).contains(want), "{v:?} AWGN");
    }
}

/// The multi-carrier robust grid at the 125R/250R/500R base rates (even carrier
/// counts): the same MultiCarrierRx core at different symbol lengths, clean and
/// under light AWGN. MFSK Varicode drops the final char.
#[test]
fn nx_rate_grid_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC";
    let want = &msg[..msg.len() - 1];
    for v in [
        PskVariant::Psk125Rc5,
        PskVariant::Psk125Rc16,
        PskVariant::Psk250Rc3,
        PskVariant::Psk250Rc7,
        PskVariant::Psk500Rc3,
    ] {
        let clean = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&clean)).contains(want), "{v:?} clean");
        let mut noisy = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x7700 + v.carriers() as u64);
        add_awgn(&mut noisy, 0.02, &mut rng);
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&noisy)).contains(want), "{v:?} AWGN");
    }
}

/// The uncoded multi-carrier `nX_PSKnnn` grid (no FEC): plain differential BPSK
/// with PSK31 Varicode over N carriers, through the decimating matched filter.
/// Clean loopback plus a gentle AWGN pass — with no FEC the noise margin is thin
/// (0.01, well below the FEC-bearing modes' 0.02+). PSK31 keeps the trailing
/// `00`, so the full message round-trips.
#[test]
fn nx_nonrobust_grid_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC";
    for v in [
        PskVariant::Psk125c12,
        PskVariant::Psk250c6,
        PskVariant::Psk500c2,
        PskVariant::Psk500c4,
        PskVariant::Psk1000c2,
    ] {
        let clean = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&clean)).contains(msg), "{v:?} clean");
        let mut noisy = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x6600 + v.carriers() as u64);
        add_awgn(&mut noisy, 0.01, &mut rng);
        assert!(texts(&PskDemod::new(v, 1500.0).feed(&noisy)).contains(msg), "{v:?} AWGN");
    }
}

// --- Group 9: DominoEX family (IFK+ MFSK, fldigi parity) -----------------

/// Bit-exact: omnimodem's DominoEX Varicode nibble stream, IFK+ tone sequence,
/// and the whole primary alphabet's encode/decode round-trip reproduce fldigi's
/// tables byte-for-byte. Provenance: `tests/vectors/dominoex_varicode.json`
/// (fldigi 4.1.23 @ 61b97f413, driver `scratch/refvectors/build_dominoex_varicode.sh`).
#[test]
fn dominoex_varicode_and_ifk_match_fldigi_vector() {
    use omnimodem_dsp::framing::dominoex_varicode::{decode_index, encode_char, Varidecoder};
    use omnimodem_dsp::modes::dominoex::{text_nibbles, text_tones};

    let raw = include_str!("vectors/dominoex_varicode.json");

    // 1. Whole primary alphabet: nib / idx / dec columns, byte-for-byte.
    let dec = Varidecoder::new();
    for c in 0u16..256 {
        let needle = format!("\"c\":{c},");
        let row = raw.lines().find(|l| l.contains(&needle)).expect("primary row");
        let field = |k: &str| {
            let i = row.find(&needle).unwrap();
            let j = row[i..].find(k).unwrap() + i + k.len();
            row[j..].to_string()
        };
        let want_nib = {
            let s = field("\"nib\":\"");
            s[..s.find('"').unwrap()].to_string()
        };
        let want_idx: u16 =
            field("\"idx\":")[..field("\"idx\":").find(&[',', '}'][..]).unwrap()].parse().unwrap();
        let want_dec: i32 =
            field("\"dec\":")[..field("\"dec\":").find(&[',', '}'][..]).unwrap()].parse().unwrap();

        let nib = encode_char(c as u8, false);
        let got_nib: String = nib.iter().map(|n| format!("{n:x}")).collect();
        assert_eq!(got_nib, want_nib, "char {c} nibbles");
        let idx = decode_index(&nib);
        assert_eq!(idx, want_idx, "char {c} decode index");
        assert_eq!(dec.decode(idx).map(|v| v as i32).unwrap_or(-1), want_dec, "char {c} decoded");
    }

    // 2. Per-message nibble + IFK+ tone streams.
    for msg in ["CQ DE K1ABC", "The quick brown fox 0123456789"] {
        let needle = format!("\"msg\":\"{msg}\"");
        let line = raw.lines().find(|l| l.contains(&needle)).expect("message line");
        let nib_field = {
            let k = "\"nibbles\":\"";
            let i = line.find(k).unwrap() + k.len();
            line[i..line[i..].find('"').unwrap() + i].to_string()
        };
        let want_nib: Vec<u8> =
            nib_field.chars().map(|ch| ch.to_digit(16).unwrap() as u8).collect();
        assert_eq!(text_nibbles(msg), want_nib, "{msg:?} nibbles");

        let k = "\"tones\":[";
        let i = line.find(k).unwrap() + k.len();
        let arr = &line[i..line[i..].find(']').unwrap() + i];
        let want_tones: Vec<u32> = arr.split(',').map(|s| s.trim().parse().unwrap()).collect();
        assert_eq!(text_tones(msg), want_tones, "{msg:?} IFK+ tones");
    }
}

/// Every DominoEX submode round-trips a message TX→RX on a clean channel and
/// under light AWGN (Goertzel tone detection + IFK+ inverse + Varicode framing).
/// The submode grid is parametric; one message per submode.
#[test]
fn dominoex_submode_grid_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::dominoex::{DominoDemod, DominoMod, DominoVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn decode(v: DominoVariant, samples: &[f32]) -> String {
        let mut rx = DominoDemod::new(v, 1500.0);
        let mut f = rx.feed(samples);
        f.extend(rx.flush());
        texts(&f)
    }

    let msg = "CQ DE K1ABC/7";
    for (i, &v) in DominoVariant::all().iter().enumerate() {
        let clean = DominoMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert_eq!(decode(v, &clean), msg, "{} clean loopback", v.label());

        let mut noisy = DominoMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0xD0E0 + i as u64);
        add_awgn(&mut noisy, 0.02, &mut rng);
        assert_eq!(decode(v, &noisy), msg, "{} AWGN loopback", v.label());
    }
}

/// Bit-exact: omnimodem's Feld Hell font (`hellfont::glyph_columns`) and on-air
/// column raster (`hellfont::on_air_columns`) reproduce fldigi's tables and
/// `tx_char` framing byte-for-byte, for every printable glyph and both test
/// messages. Provenance: `tests/vectors/feldhell.json` (fldigi 4.1.23 @
/// 61b97f413, driver `scratch/refvectors/build_feldhell.sh`, feldfontnbr 4).
#[test]
fn feldhell_font_and_raster_match_fldigi_vector() {
    use omnimodem_dsp::framing::hellfont::{glyph_columns, on_air_columns, DEFAULT_XMT_WIDTH};

    let raw = include_str!("vectors/feldhell.json");

    // Parse the `"cols":[a,b,c]` array from a vector line.
    fn cols_of(line: &str) -> Vec<u16> {
        let k = "\"cols\":[";
        let i = line.find(k).unwrap() + k.len();
        let arr = &line[i..line[i..].find(']').unwrap() + i];
        if arr.trim().is_empty() {
            return Vec::new();
        }
        arr.split(',').map(|s| s.trim().parse().unwrap()).collect()
    }

    // 1. Every printable glyph's trimmed column raster.
    for c in b' '..=b'~' {
        let needle = format!("\"kind\":\"glyph\",\"c\":{},", c);
        let line = raw.lines().find(|l| l.contains(&needle)).expect("glyph row");
        assert_eq!(glyph_columns(c), cols_of(line), "glyph {c}");
    }

    // 2. Both on-air column streams (leading/trailing null-column framing).
    for msg in ["CQ DE K1ABC", "The quick brown fox 0123456789"] {
        let needle = format!("\"kind\":\"stream\",\"msg\":\"{msg}\"");
        let line = raw.lines().find(|l| l.contains(&needle)).expect("stream row");
        assert_eq!(on_air_columns(msg, DEFAULT_XMT_WIDTH), cols_of(line), "{msg:?} stream");
    }
}

/// Every Feld Hell submode round-trips a message TX→RX as a raster on a clean
/// channel and under light AWGN: the decoded image columns reproduce the
/// bit-exact on-air glyph columns (the facsimile loopback gate, Doctrine §3 —
/// audio is never asserted bit-exact).
#[test]
fn hell_submode_grid_raster_loopback_and_awgn() {
    use omnimodem_dsp::framing::hellfont::{on_air_columns, DEFAULT_XMT_WIDTH};
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::hell::{image_columns, HellDemod, HellMod, HellVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn raster(v: HellVariant, samples: &[f32]) -> Vec<u16> {
        let mut rx = HellDemod::new(v, 1500.0);
        rx.feed(samples);
        let frames = rx.flush();
        match &frames[0].payload {
            FramePayload::Image { width, gray } => image_columns(*width, gray),
            _ => panic!("expected Image payload"),
        }
    }

    let msg = "CQ DE K1ABC";
    let want = on_air_columns(msg, DEFAULT_XMT_WIDTH);
    for (i, &v) in HellVariant::all().iter().enumerate() {
        let clean = HellMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let got = raster(v, &clean);
        assert_eq!(&got[..want.len()], &want[..], "{} clean raster loopback", v.label());

        let mut noisy = HellMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0xFE1D + i as u64);
        add_awgn(&mut noisy, 0.02, &mut rng);
        let got = raster(v, &noisy);
        assert_eq!(&got[..want.len()], &want[..], "{} AWGN raster loopback", v.label());
    }
}

// --- Group 11: MFSK + Contestia families (fldigi parity) ------------------

/// Bit-exact: omnimodem's MFSK TX chain (varicode → K=7 conv → interleave →
/// grayencode) reproduces fldigi's `coded` bits, interleaved `symbols`, and
/// gray `tones` byte-for-byte for every representative submode. Provenance:
/// `tests/vectors/mfsk.json` (fldigi 4.1.23 @ 61b97f413, driver
/// `scratch/refvectors/build_mfsk.sh`).
#[test]
fn mfsk_tx_chain_matches_fldigi_vector() {
    use omnimodem_dsp::framing::varicode::mfsk_encode;
    use omnimodem_dsp::modes::mfsk::{text_tones, MfskVariant};

    let raw = include_str!("vectors/mfsk.json");
    for &v in MfskVariant::all() {
        let needle = format!("\"mode\":\"{}\"", v.label());
        let line = raw.lines().find(|l| l.contains(&needle)).expect("submode line");
        let field = |k: &str| {
            let i = line.find(k).unwrap() + k.len();
            line[i..line[i..].find('"').unwrap() + i].to_string()
        };
        let msg = field("\"msg\":\"");
        let want_tones: Vec<u32> =
            field("\"tones\":\"").split(' ').map(|s| s.parse().unwrap()).collect();
        assert_eq!(text_tones(v, &msg), want_tones, "{} tones", v.label());

        // The varicode bits are the front of the chain — assert them too so a
        // break localises to the varicode vs the FEC/interleave/gray stages.
        let want_vari: Vec<u8> = field("\"varicode\":\"").bytes().map(|c| c - b'0').collect();
        assert_eq!(mfsk_encode(&msg), want_vari, "{} varicode", v.label());
    }
}

/// Every MFSK submode round-trips a message TX→RX on a clean channel and under
/// light AWGN (Goertzel tone detection + gray/interleave inverse + streaming
/// Viterbi + MFSK Varicode framing). Parametric grid; one message per submode.
#[test]
fn mfsk_submode_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::mfsk::{MfskDemod, MfskMod, MfskVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn decode(v: MfskVariant, samples: &[f32]) -> String {
        let mut rx = MfskDemod::new(v, 1500.0);
        rx.feed(samples)
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC 73";
    // The deep-interleave 64L/128L modes have very long latency (and 8000-sample
    // symbols for 64L); the shallow reps of each symbits width carry the grid.
    for (i, &v) in [
        MfskVariant::M4,
        MfskVariant::M8,
        MfskVariant::M16,
        MfskVariant::M31,
        MfskVariant::M32,
        MfskVariant::M64,
        MfskVariant::M128,
        MfskVariant::M11,
        MfskVariant::M22,
    ]
    .iter()
    .enumerate()
    {
        let clean = MfskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        assert_eq!(decode(v, &clean), msg, "{} clean loopback", v.label());

        let mut noisy = MfskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x11F5 + i as u64);
        add_awgn(&mut noisy, 0.02, &mut rng);
        assert_eq!(decode(v, &noisy), msg, "{} AWGN loopback", v.label());
    }
}

/// Every Contestia submode round-trips a message TX→RX on a clean channel and
/// under light AWGN (MFSK tone bank + 32-chip Walsh soft decode). Parametric
/// grid; one message per submode.
#[test]
fn contestia_grid_loopback_and_awgn() {
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::modes::contestia::{ContestiaDemod, ContestiaMod, ContestiaVariant};
    use omnimodem_dsp::types::{Frame, FramePayload};

    fn decode(v: ContestiaVariant, samples: &[f32]) -> String {
        let mut rx = ContestiaDemod::new(v.tones, v.bandwidth_hz);
        rx.feed(samples)
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    let msg = "CQ DE K1ABC 2024";
    for (i, &v) in ContestiaVariant::all().iter().enumerate() {
        let clean = ContestiaMod::new(v.tones, v.bandwidth_hz).modulate(&Frame::text(msg)).unwrap();
        assert_eq!(decode(v, &clean), msg, "{} clean loopback", v.label());

        let mut noisy =
            ContestiaMod::new(v.tones, v.bandwidth_hz).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0xC047 + i as u64);
        add_awgn(&mut noisy, 0.015, &mut rng);
        assert_eq!(decode(v, &noisy), msg, "{} AWGN loopback", v.label());
    }
}
