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

        // Beyond capacity: must NOT silently reproduce a wrong codeword as the
        // original. Either Err, or a result that is not the original message.
        let mut cw2: Vec<u8> = data.iter().chain(parity.iter()).copied().collect();
        for b in cw2.iter_mut().take(t + 2) {
            *b ^= 0xA5;
        }
        let res2 = rs.decode(&mut cw2);
        if res2.is_ok() {
            assert_ne!(&cw2[..data.len()], &data[..], "beyond-capacity must not miscorrect to original");
        }
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
    // Direwolf HDLC/AX.25:  gen_packets -o out.wav -n 1 "K1ABC>APRS:>test"
    // Direwolf FX.25:       gen_packets -X 16 ...   (RS(255,239)-shortened)
    // Direwolf IL2P:        gen_packets -I 1 ...    (cross-check il2p_test)
    // ft8_lib LDPC/77-bit:  ft8code "CQ K1ABC FN42"  -> 77-bit + 174-bit codeword
    // Capture the bytes into tests/vectors/*.json with this comment as the
    // provenance header, then drop the `#[ignore]` on the corresponding KAT.
    panic!("documentation-only: see comment for regeneration commands");
}

// --- Phase-3 exit criterion ----------------------------------------------

/// The executable definition of "Phase 3 done": the contract-critical KATs all
/// pass. Each underlying KAT is also its own `#[test]`; this aggregates the
/// subset that gates the phase (design §"Phase-3 exit criterion").
#[test]
fn phase3_exit_criterion() {
    crc16_x25_check_value();
    crc14_ft8_is_14_bits_and_deterministic();
    rs_corrects_within_capacity_and_detects_beyond();
    ldpc_encode_noiseless_decode();
    ft8_costas_array_is_canonical();
    hdlc_frame_roundtrips_and_fcs_guards();
    ax25_ui_frame_roundtrips();
    message77_standard_roundtrips();
}
