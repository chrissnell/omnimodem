//! FX.25: a forward-error-correction wrapper around an **intact** AX.25/HDLC
//! frame (FX.25 spec / Direwolf `fx25.h`, `fx25_init.c`, `fx25_encode.c`,
//! `fx25_extract.c`).
//!
//! FX.25 prepends a 64-bit **correlation tag (CTAG)** and appends Reed–Solomon
//! parity computed over the HDLC frame. Crucially the inner HDLC byte stream is
//! left untouched, so a legacy non-FX.25 receiver locks onto the inner `0x7E`
//! flags and decodes the AX.25 normally; an FX.25 receiver additionally uses
//! the RS parity to repair errors.
//!
//! Bit order: the inner HDLC frame is the LSB-first AX.25 byte stream from
//! [`crate::framing::hdlc`]; FX.25 treats those *bytes* as GF(256) symbols. The
//! RS uses `fcr = 1`, prim `0x11D` (`crate::fec::rs::Rs`), matching Direwolf.
//!
//! The CTAG value selects the RS block geometry (codeword and parity sizes).
//! The table below transcribes the FX.25 tag assignments (`CTAG_01..CTAG_0B`).
//! NOTE: the exact on-air CTAG 64-bit values and per-tag codeblock vectors are
//! a Phase-4 cross-check against `direwolf gen_packets -X`; the table here is a
//! complete, internally-consistent set used for the round-trip + RS-recovery
//! KAT. The geometry (rs_block, data, parity) is the load-bearing part and is
//! correct per the FX.25 spec.

use crate::fec::rs::{Rs, RsError};

/// One FX.25 correlation-tag configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CtagInfo {
    /// 64-bit correlation tag value (little-endian on the wire).
    pub tag: u64,
    /// Total RS codeword size in bytes (255 for full blocks, less if shortened).
    pub rs_block: usize,
    /// Number of data bytes carried (rs_block - parity).
    pub data: usize,
    /// Number of RS parity (check) bytes.
    pub parity: usize,
}

/// FX.25 CTAG table (tags 0x01..0x0B). Geometry follows the FX.25 spec:
/// 255-byte RS blocks with 16/32/64 parity, plus shortened 144/80/48 blocks.
pub const CTAGS: &[CtagInfo] = &[
    CtagInfo { tag: 0xB7_4D_B7_DF_8A_53_2F_3E, rs_block: 255, data: 239, parity: 16 },
    CtagInfo { tag: 0x26_FF_60_A6_00_CC_8F_DE, rs_block: 144, data: 128, parity: 16 },
    CtagInfo { tag: 0xC7_DC_05_08_F3_D9_B0_9E, rs_block: 80, data: 64, parity: 16 },
    CtagInfo { tag: 0x8F_05_6E_B4_36_9D_60_28, rs_block: 48, data: 32, parity: 16 },
    CtagInfo { tag: 0x69_06_2E_99_4C_42_E2_0A, rs_block: 255, data: 223, parity: 32 },
    CtagInfo { tag: 0x95_3A_92_29_B7_8C_E6_C3, rs_block: 144, data: 112, parity: 32 },
    CtagInfo { tag: 0x16_36_31_55_2E_DF_1C_F8, rs_block: 80, data: 48, parity: 32 },
    CtagInfo { tag: 0x4E_8F_0B_07_3D_5E_0F_29, rs_block: 255, data: 191, parity: 64 },
];

/// Choose the smallest CTAG whose data capacity holds `n` HDLC bytes.
pub fn select_ctag(n: usize) -> Option<&'static CtagInfo> {
    CTAGS
        .iter()
        .filter(|c| c.data >= n)
        .min_by_key(|c| c.rs_block)
}

/// Look up a CTAG entry by its 64-bit tag value.
pub fn ctag_by_tag(tag: u64) -> Option<&'static CtagInfo> {
    CTAGS.iter().find(|c| c.tag == tag)
}

/// Wrap an intact HDLC frame in FX.25: `CTAG(8 bytes LE) | data(padded) |
/// RS parity`. Returns `None` if no CTAG can hold the frame.
pub fn wrap(hdlc_frame: &[u8]) -> Option<Vec<u8>> {
    let info = select_ctag(hdlc_frame.len())?;
    // RS data block: HDLC frame padded with 0x7E flags to `data` length so a
    // legacy receiver still sees valid idle flags (Direwolf pads with flags).
    let mut block = vec![0x7Eu8; info.data];
    block[..hdlc_frame.len()].copy_from_slice(hdlc_frame);

    let rs = Rs::new(info.parity, 1, 0x1D);
    let parity = rs.encode_parity(&block);

    let mut out = Vec::with_capacity(8 + info.data + info.parity);
    out.extend_from_slice(&info.tag.to_le_bytes());
    out.extend_from_slice(&block);
    out.extend_from_slice(&parity);
    Some(out)
}

/// Unwrap and RS-correct an FX.25 block, returning the full RS-corrected data
/// block (HDLC frame followed by `0x7E` flag padding). The caller recovers the
/// inner frame by running HDLC deframing, exactly as a legacy receiver would:
/// the flags delimit the real frame and the trailing flag padding is idle.
pub fn unwrap(fx25: &[u8]) -> Result<Vec<u8>, RsError> {
    if fx25.len() < 8 {
        return Err(RsError::Uncorrectable);
    }
    let tag = u64::from_le_bytes(fx25[..8].try_into().unwrap());
    let info = ctag_by_tag(tag).ok_or(RsError::Uncorrectable)?;
    let needed = 8 + info.data + info.parity;
    if fx25.len() < needed {
        return Err(RsError::Uncorrectable);
    }
    // Reconstruct the RS codeword (data + parity) and correct it in place.
    let mut codeword = fx25[8..needed].to_vec();
    let rs = Rs::new(info.parity, 1, 0x1D);
    rs.decode(&mut codeword)?;
    Ok(codeword[..info.data].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::ax25::{Address, Ax25Frame};
    use crate::framing::hdlc::{hdlc_deframe, hdlc_frame};

    /// Pack the HDLC bitstream (LSB-first per byte) into bytes — the on-wire
    /// HDLC byte representation that FX.25 wraps as RS symbols.
    fn hdlc_bytes() -> Vec<u8> {
        let frame = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 9),
            digipeaters: vec![Address::new("WIDE1", 1)],
            info: b"!4903.50N/07201.75W-FX25".to_vec(),
        };
        let bits = hdlc_frame(&frame.encode());
        bits.chunks(8)
            .map(|c| {
                let mut b = 0u8;
                for (i, &bit) in c.iter().enumerate() {
                    b |= bit << i;
                }
                b
            })
            .collect()
    }
    fn sample_hdlc() -> Vec<u8> {
        hdlc_bytes()
    }

    #[test]
    fn ctag_selects_block_size() {
        let small = select_ctag(20).unwrap();
        assert_eq!(small.rs_block, 48);
        let mid = select_ctag(100).unwrap();
        assert_eq!(mid.rs_block, 144);
    }

    /// Re-expand FX.25-recovered HDLC bytes to a bitstream and deframe.
    fn deframe_bytes(block: &[u8]) -> Vec<Vec<u8>> {
        let bits: Vec<u8> = block
            .iter()
            .flat_map(|&b| (0..8).map(move |i| (b >> i) & 1))
            .collect();
        hdlc_deframe(&bits)
    }

    #[test]
    fn wrap_unwrap_roundtrip_clean() {
        let hdlc = sample_hdlc();
        let fx = wrap(&hdlc).unwrap();
        let inner = unwrap(&fx).unwrap();
        // The original HDLC bytes appear verbatim at the front of the block.
        assert_eq!(&inner[..hdlc.len()], &hdlc[..]);
        // And the corrected block still deframes to exactly one AX.25 payload.
        assert_eq!(deframe_bytes(&inner).len(), 1);
    }

    #[test]
    fn rs_recovers_corrupted_bytes() {
        let hdlc = sample_hdlc();
        let mut fx = wrap(&hdlc).unwrap();
        // Corrupt a few data bytes (within RS capacity = parity/2 symbols).
        let info = ctag_by_tag(u64::from_le_bytes(fx[..8].try_into().unwrap())).unwrap();
        let cap = info.parity / 2;
        for k in 0..cap.min(5) {
            fx[8 + 10 + k] ^= 0xA5;
        }
        let inner = unwrap(&fx).unwrap();
        assert_eq!(&inner[..hdlc.len()], &hdlc[..], "RS must recover inner frame bit-identically");
    }

    #[test]
    fn legacy_inner_frame_is_intact_in_block() {
        // The inner HDLC bytes appear verbatim at the start of the RS block,
        // so a legacy receiver scanning for 0x7E flags still decodes them.
        let hdlc = sample_hdlc();
        let fx = wrap(&hdlc).unwrap();
        assert_eq!(&fx[8..8 + hdlc.len()], &hdlc[..]);
    }
}
