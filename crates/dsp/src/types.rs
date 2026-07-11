//! Core data types shared by every block.
//!
//! `Llr` convention (locked, do not flip per block): `L = ln(P(0)/P(1))`.
//! Positive => bit 0 more likely; `hard()` slices `bit = (L < 0.0)`.

use num_complex::Complex32;

/// Real audio sample, normalized to `[-1.0, 1.0)`.
pub type Sample = f32;

/// Complex baseband sample.
pub type Cplx = Complex32;

/// A per-bit log-likelihood ratio, `ln(P(bit=0)/P(bit=1))`.
pub type Llr = f32;

/// A run of soft bits carried across the demapper -> FEC boundary.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SoftBits(pub Vec<Llr>);

impl SoftBits {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Hard-slice each LLR. `bit = (l < 0.0)`; an exact `0.0` slices to 0.
    pub fn hard(&self) -> Vec<u8> {
        self.0.iter().map(|&l| u8::from(l < 0.0)).collect()
    }
}

/// Payload-agnostic so voice/packet/text/raw modes share one `Frame` type.
#[derive(Debug, Clone, PartialEq)]
pub enum FramePayload {
    /// AX.25/HDLC or other byte-oriented packet.
    Packet(Vec<u8>),
    /// Decoded human-readable text (FT8/PSK31/RTTY/CW).
    Text(String),
    /// Raw packed 77-bit WSJT-X message (10 bytes, last 3 bits zero).
    Message77([u8; 10]),
    /// Opaque vocoder bits (Phase-5 voice modes).
    Vocoder(Vec<u8>),
    /// Raster/scanline image for facsimile modes (Hell, WEFAX, and the
    /// MFSK/THOR/IFKP/FSQ picture sub-protocols). `pixels` is row-major 8-bit
    /// samples, `channels` interleaved values per pixel: `channels == 1` is
    /// grayscale luminance, `channels == 3` is `R,G,B,R,G,B,…`. `pixels.len()`
    /// is a whole multiple of `width * channels`, so the row count is
    /// `pixels.len() / (width * channels)`.
    Image { width: u16, channels: u8, pixels: Vec<u8> },
}

impl FramePayload {
    /// Stable content hash input for dedup (ignores metadata).
    pub fn hash_into<H: std::hash::Hasher>(&self, h: &mut H) {
        use std::hash::Hash;
        match self {
            FramePayload::Packet(b) => {
                0u8.hash(h);
                b.hash(h);
            }
            FramePayload::Text(s) => {
                1u8.hash(h);
                s.hash(h);
            }
            FramePayload::Message77(m) => {
                2u8.hash(h);
                m.hash(h);
            }
            FramePayload::Vocoder(b) => {
                3u8.hash(h);
                b.hash(h);
            }
            FramePayload::Image { width, channels, pixels } => {
                4u8.hash(h);
                width.hash(h);
                channels.hash(h);
                pixels.hash(h);
            }
        }
    }
}

/// Decode metadata. The daemon attaches channel/timestamp downstream; the DSP
/// layer fills the signal-quality fields it measured.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameMeta {
    pub snr_db: Option<f32>,
    pub freq_offset_hz: Option<f32>,
    pub time_offset_s: Option<f32>,
    /// Which ensemble member / slicer produced this frame.
    pub decoder: Option<String>,
    /// Sample offset of the frame within the fed buffer (dedup key).
    pub sample_offset: u64,
    pub crc_ok: bool,
    /// Soft-decision confidence in `[0, 1]`, when the decoder measures one. ADS-B
    /// fills it with the mean per-bit eye (matched filter + decision-feedback AGC)
    /// that gates CRC-lucky false positives; other modes leave it `None`.
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub payload: FramePayload,
    pub meta: FrameMeta,
}

impl Frame {
    pub fn packet(bytes: Vec<u8>) -> Self {
        Frame { payload: FramePayload::Packet(bytes), meta: FrameMeta::default() }
    }
    pub fn text(s: impl Into<String>) -> Self {
        Frame { payload: FramePayload::Text(s.into()), meta: FrameMeta::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llr_hard_slice_uses_locked_convention() {
        // Positive => 0, negative => 1, zero => 0.
        let sb = SoftBits(vec![3.0, -3.0, 0.0, -0.001]);
        assert_eq!(sb.hard(), vec![0, 1, 0, 1]);
    }

    #[test]
    fn frame_constructors_default_meta() {
        let f = Frame::packet(vec![1, 2, 3]);
        assert!(!f.meta.crc_ok);
        assert_eq!(f.meta.sample_offset, 0);
    }

    #[test]
    fn image_payload_roundtrips_and_hashes() {
        use std::hash::Hasher;
        // 2 rows x 3 cols of 8-bit grayscale luminance.
        let img = FramePayload::Image { width: 3, channels: 1, pixels: vec![0, 128, 255, 1, 2, 3] };
        let f = Frame { payload: img.clone(), meta: FrameMeta::default() };
        assert_eq!(f.payload, img);
        if let FramePayload::Image { width, channels, pixels } = &f.payload {
            let stride = *width as usize * *channels as usize;
            assert_eq!(pixels.len() % stride, 0);
            assert_eq!(pixels.len() / stride, 2); // 2 rows
        } else {
            panic!("expected Image");
        }
        // The new variant participates in the dedup hash (distinct tag 4u8).
        let mut ha = std::collections::hash_map::DefaultHasher::new();
        img.hash_into(&mut ha);
        let mut hb = std::collections::hash_map::DefaultHasher::new();
        FramePayload::Image { width: 3, channels: 1, pixels: vec![0, 128, 255, 1, 2, 4] }
            .hash_into(&mut hb);
        assert_ne!(ha.finish(), hb.finish());
    }

    #[test]
    fn image_payload_color_is_distinct_from_grayscale() {
        use std::hash::Hasher;
        // Same buffer, different channel interpretation must hash differently
        // (2px RGB vs 6px gray) and compare unequal.
        let rgb = FramePayload::Image { width: 2, channels: 3, pixels: vec![10, 20, 30, 40, 50, 60] };
        let gray = FramePayload::Image { width: 6, channels: 1, pixels: vec![10, 20, 30, 40, 50, 60] };
        assert_ne!(rgb, gray);
        let mut ha = std::collections::hash_map::DefaultHasher::new();
        rgb.hash_into(&mut ha);
        let mut hb = std::collections::hash_map::DefaultHasher::new();
        gray.hash_into(&mut hb);
        assert_ne!(ha.finish(), hb.finish());
    }
}
