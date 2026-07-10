//! RTL-SDR raw-IQ conversion: interleaved unsigned-8-bit I/Q (centered at
//! 127.5, as `rtl_tcp` streams it) to normalized complex baseband.

use crate::types::Cplx;

/// Convert interleaved `u8` I/Q pairs to complex baseband in ~[-1.0, 1.0).
/// A trailing odd byte (a split pair across TCP reads) is ignored; callers
/// carry it into the next call's buffer.
pub fn u8_iq_to_cplx(bytes: &[u8]) -> Vec<Cplx> {
    bytes
        .chunks_exact(2)
        .map(|p| {
            let i = (p[0] as f32 - 127.5) / 127.5;
            let q = (p[1] as f32 - 127.5) / 127.5;
            Cplx::new(i, q)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_bytes_map_near_zero() {
        // 127 and 128 straddle the 127.5 DC point → magnitude well under 0.01.
        let out = u8_iq_to_cplx(&[127, 128]);
        assert_eq!(out.len(), 1);
        assert!(out[0].norm() < 0.01, "got {:?}", out[0]);
    }

    #[test]
    fn extremes_map_to_full_scale() {
        let out = u8_iq_to_cplx(&[255, 0]);
        assert!((out[0].re - 1.0).abs() < 0.01);
        assert!((out[0].im + 1.0).abs() < 0.02); // 0 → (0-127.5)/127.5 ≈ -1.0
    }

    #[test]
    fn odd_trailing_byte_is_dropped() {
        let out = u8_iq_to_cplx(&[200, 60, 10]);
        assert_eq!(out.len(), 1);
    }
}
