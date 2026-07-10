//! Pure KISS framing — no I/O. See the protocol table in the Phase 6 plan.

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// KISS command for a data frame on port 0: `(port << 4) | cmd` = `0x00`.
const CMD_DATA: u8 = 0x00;

/// Encode `payload` (a complete AX.25 frame) as a KISS data frame, ready to
/// write to a host socket: `FEND, 0x00, <escaped payload>, FEND`.
pub fn encode_data_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.push(FEND);
    out.push(CMD_DATA);
    for &b in payload {
        match b {
            FEND => out.extend_from_slice(&[FESC, TFEND]),
            FESC => out.extend_from_slice(&[FESC, TFESC]),
            other => out.push(other),
        }
    }
    out.push(FEND);
    out
}

/// A decoded KISS frame: the split command byte and the unescaped data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KissFrame {
    pub port: u8,
    pub cmd: u8,
    pub data: Vec<u8>,
}

impl KissFrame {
    /// True if this is a data frame (command low-nibble 0) — the only kind we
    /// transmit. Parameter/exit commands return false and are ignored.
    pub fn is_data(&self) -> bool {
        self.cmd == 0x0
    }
}

/// Streaming KISS decoder. Feed arbitrary byte chunks; get back any frames that
/// completed. Holds partial-frame and escape state across calls.
#[derive(Default)]
pub struct KissDecoder {
    buf: Vec<u8>,
    in_frame: bool,
    escaped: bool,
}

impl KissDecoder {
    pub fn new() -> Self {
        KissDecoder::default()
    }

    /// Feed bytes; return every frame that completed in this chunk.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<KissFrame> {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                FEND => {
                    if self.in_frame && !self.buf.is_empty() {
                        if let Some(f) = self.take_frame() {
                            out.push(f);
                        }
                    }
                    // Either way a FEND resets to "between frames".
                    self.buf.clear();
                    self.escaped = false;
                    self.in_frame = true;
                }
                FESC if self.in_frame => self.escaped = true,
                _ if self.in_frame => {
                    let byte = if self.escaped {
                        self.escaped = false;
                        match b {
                            TFEND => FEND,
                            TFESC => FESC,
                            other => other, // tolerate malformed escape
                        }
                    } else {
                        b
                    };
                    self.buf.push(byte);
                }
                _ => {} // bytes before the first FEND are noise
            }
        }
        out
    }

    /// Split the accumulated bytes into command + data. Caller guarantees
    /// `self.buf` is non-empty.
    fn take_frame(&mut self) -> Option<KissFrame> {
        let cmd_byte = *self.buf.first()?;
        let data = self.buf[1..].to_vec();
        Some(KissFrame {
            port: cmd_byte >> 4,
            cmd: cmd_byte & 0x0F,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_plain_data_frame() {
        // FEND, cmd=0x00 (data, port 0), payload, FEND
        assert_eq!(encode_data_frame(&[0x11, 0x22, 0x33]), vec![0xC0, 0x00, 0x11, 0x22, 0x33, 0xC0]);
    }

    #[test]
    fn escapes_fend_and_fesc_in_payload() {
        assert_eq!(encode_data_frame(&[0xC0]), vec![0xC0, 0x00, 0xDB, 0xDC, 0xC0]);
        assert_eq!(encode_data_frame(&[0xDB]), vec![0xC0, 0x00, 0xDB, 0xDD, 0xC0]);
        assert_eq!(
            encode_data_frame(&[0xDB, 0xC0]),
            vec![0xC0, 0x00, 0xDB, 0xDD, 0xDB, 0xDC, 0xC0]
        );
    }

    #[test]
    fn encodes_an_empty_payload() {
        assert_eq!(encode_data_frame(&[]), vec![0xC0, 0x00, 0xC0]);
    }

    #[test]
    fn decodes_a_single_frame_across_chunked_input() {
        let mut d = KissDecoder::new();
        // FEND, cmd=0x00, [0x11,0x22], FEND — fed in two chunks.
        let mut frames = d.push(&[0xC0, 0x00, 0x11]);
        assert!(frames.is_empty());
        frames = d.push(&[0x22, 0xC0]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].port, 0);
        assert_eq!(frames[0].cmd, 0x0);
        assert_eq!(frames[0].data, vec![0x11, 0x22]);
    }

    #[test]
    fn unescapes_transposed_bytes() {
        let mut d = KissDecoder::new();
        let frames = d.push(&[0xC0, 0x00, 0xDB, 0xDC, 0xDB, 0xDD, 0xC0]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, vec![0xC0, 0xDB]);
    }

    #[test]
    fn ignores_empty_frames_and_back_to_back_fends() {
        let mut d = KissDecoder::new();
        // Leading/duplicate FENDs are delimiters, not zero-length frames.
        let frames = d.push(&[0xC0, 0xC0, 0x00, 0x41, 0xC0]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, vec![0x41]);
    }

    #[test]
    fn round_trips_encode_then_decode() {
        let payload = vec![0x00, 0xC0, 0xDB, 0xFF, 0x7E];
        let wire = encode_data_frame(&payload);
        let mut d = KissDecoder::new();
        let frames = d.push(&wire);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, payload);
    }
}
