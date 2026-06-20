//! AX.25 UI-frame (APRS convention) encode/decode.
//!
//! An AX.25 UI frame carries: an address field (destination, source, then up
//! to 8 digipeaters), a control byte `0x03` (UI, unnumbered information), a PID
//! `0xF0` (no layer-3), and the information field.
//!
//! Address encoding: each callsign is 6 ASCII characters (space-padded),
//! **left-shifted by one bit** (`c << 1`), followed by an SSID byte. The SSID
//! byte is `0b0_RRR_SSSS_0`-style: bit0 is the HDLC address-extension bit (1 on
//! the *last* address subfield, 0 otherwise), bits1..4 are the 4-bit SSID,
//! bits5..6 are reserved (set to 1 by convention), bit7 is the C/H bit.
//!
//! Bit order: AX.25 rides inside HDLC, which is **LSB-first on the wire**.
//! This module produces/consumes the *byte* sequence (address + control + PID
//! + info) that [`crate::framing::hdlc`] then serialises LSB-first and FCS-wraps.

/// One AX.25 address subfield: a callsign plus SSID.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Address {
    pub call: String,
    pub ssid: u8,
}

impl Address {
    pub fn new(call: &str, ssid: u8) -> Self {
        Address { call: call.to_ascii_uppercase(), ssid: ssid & 0x0F }
    }

    /// Encode to 7 bytes: 6 shifted callsign chars + SSID byte.
    fn encode(&self, last: bool, has_been_repeated: bool) -> [u8; 7] {
        let mut out = [0u8; 7];
        let bytes = self.call.as_bytes();
        for i in 0..6 {
            let c = if i < bytes.len() { bytes[i] } else { b' ' };
            out[i] = c << 1;
        }
        // SSID byte: H | RR(=11) | SSID(4) | ext-bit
        let h = if has_been_repeated { 0x80 } else { 0 };
        out[6] = h | 0x60 | ((self.ssid & 0x0F) << 1) | (last as u8);
        out
    }

    fn decode(raw: &[u8]) -> Address {
        let call: String = raw[..6]
            .iter()
            .map(|&b| (b >> 1) as char)
            .collect::<String>()
            .trim_end()
            .to_string();
        let ssid = (raw[6] >> 1) & 0x0F;
        Address { call, ssid }
    }

    /// True if this subfield is the last (address-extension bit set).
    fn is_last(raw: &[u8]) -> bool {
        raw[6] & 0x01 != 0
    }
}

/// An AX.25 UI frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ax25Frame {
    pub dest: Address,
    pub source: Address,
    pub digipeaters: Vec<Address>,
    pub info: Vec<u8>,
}

const CONTROL_UI: u8 = 0x03;
const PID_NO_L3: u8 = 0xF0;

impl Ax25Frame {
    /// Encode to the AX.25 byte sequence (address + control + PID + info),
    /// ready to be HDLC-framed.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // Address field: dest, source, digis. Order on wire is dest then source.
        let n_digi = self.digipeaters.len();
        out.extend_from_slice(&self.dest.encode(false, false));
        let src_last = n_digi == 0;
        out.extend_from_slice(&self.source.encode(src_last, false));
        for (i, d) in self.digipeaters.iter().enumerate() {
            let last = i + 1 == n_digi;
            out.extend_from_slice(&d.encode(last, false));
        }
        out.push(CONTROL_UI);
        out.push(PID_NO_L3);
        out.extend_from_slice(&self.info);
        out
    }

    /// Decode an AX.25 byte sequence (as recovered from an HDLC frame).
    pub fn decode(bytes: &[u8]) -> Option<Ax25Frame> {
        if bytes.len() < 7 * 2 + 2 {
            return None;
        }
        let mut addrs = Vec::new();
        let mut pos = 0;
        loop {
            if pos + 7 > bytes.len() {
                return None;
            }
            let raw = &bytes[pos..pos + 7];
            let last = Address::is_last(raw);
            addrs.push(Address::decode(raw));
            pos += 7;
            if last {
                break;
            }
            if addrs.len() > 10 {
                return None; // runaway / malformed
            }
        }
        if addrs.len() < 2 || pos + 2 > bytes.len() {
            return None;
        }
        let control = bytes[pos];
        let pid = bytes[pos + 1];
        if control != CONTROL_UI || pid != PID_NO_L3 {
            return None;
        }
        let info = bytes[pos + 2..].to_vec();
        let dest = addrs[0].clone();
        let source = addrs[1].clone();
        let digipeaters = addrs[2..].to_vec();
        Some(Ax25Frame { dest, source, digipeaters, info })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::hdlc::{hdlc_deframe, hdlc_frame};

    #[test]
    fn aprs_position_roundtrip() {
        // A real APRS position report: source N0CALL-9 to APRS via WIDE1-1.
        let frame = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 9),
            digipeaters: vec![Address::new("WIDE1", 1)],
            info: b"!4903.50N/07201.75W-Test".to_vec(),
        };
        let bytes = frame.encode();
        let back = Ax25Frame::decode(&bytes).expect("decode");
        assert_eq!(back, frame);
    }

    #[test]
    fn callsign_is_left_shifted() {
        let frame = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 0),
            digipeaters: vec![],
            info: vec![],
        };
        let bytes = frame.encode();
        // First dest char 'A' = 0x41, shifted left = 0x82.
        assert_eq!(bytes[0], b'A' << 1);
        // Control/PID after two 7-byte addresses.
        assert_eq!(bytes[14], CONTROL_UI);
        assert_eq!(bytes[15], PID_NO_L3);
    }

    #[test]
    fn extension_bit_marks_last_address() {
        let frame = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 0),
            digipeaters: vec![],
            info: vec![],
        };
        let bytes = frame.encode();
        assert_eq!(bytes[6] & 1, 0, "dest is not last");
        assert_eq!(bytes[13] & 1, 1, "source is last (no digis)");
    }

    #[test]
    fn through_hdlc_roundtrip() {
        // Full path: AX.25 -> HDLC bits -> deframe -> AX.25.
        let frame = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 9),
            digipeaters: vec![Address::new("WIDE2", 2)],
            info: b">Status text".to_vec(),
        };
        let bits = hdlc_frame(&frame.encode());
        let payloads = hdlc_deframe(&bits);
        assert_eq!(payloads.len(), 1);
        assert_eq!(Ax25Frame::decode(&payloads[0]), Some(frame));
    }
}
