//! ALSA canonicalization + defensive rate/format selection. All pure; the cpal
//! backend (Task 6) feeds these the values cpal reports. Lifted from Graywolf
//! `audio/soundcard.rs` (`parse_proc_asound_cards`, `choose_stream_rate`,
//! `pick_input_sample_format`).

use super::{AudioError, MAX_SAMPLE_RATE};

/// A sample format a device advertises, ranked by how well the cheap USB codecs
/// we target honor it. I16 first: it is the native wire format and does not
/// POLLERR-loop cpal the way an ALSA-plughw-synthesized F32 does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFmt {
    I16,
    F32,
    U16,
}

impl SampleFmt {
    fn rank(self) -> u8 {
        match self {
            SampleFmt::I16 => 0,
            SampleFmt::F32 => 1,
            SampleFmt::U16 => 2,
        }
    }
}

/// Parse `/proc/asound/cards`: lines like ` 0 [Device  ]: USB-Audio - ...`
/// yield `(0, "Device")`. Indented continuation lines are ignored.
pub fn parse_proc_asound_cards(contents: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for line in contents.lines() {
        // A card header starts with optional spaces then an index digit.
        let trimmed = line.trim_start();
        if trimmed.len() == line.len() {
            continue; // not indented at all -> not a card row in this format
        }
        let mut it = trimmed.split_whitespace();
        let Some(idx) = it.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        // Next token is `[Name`; strip the bracket.
        if let Some(name_tok) = it.next() {
            let name = name_tok.trim_start_matches('[').trim_end_matches(']');
            if !name.is_empty() {
                out.push((idx, name.to_string()));
            }
        }
    }
    out
}

/// Extract the `CARD=` token from a cpal/ALSA pcm id like
/// `plughw:CARD=Device,DEV=0`. `None` for shorthand or non-ALSA names.
pub fn alsa_card_token(pcm_id: &str) -> Option<&str> {
    let after = pcm_id.split("CARD=").nth(1)?;
    Some(after.split(',').next().unwrap_or(after))
}

/// Choose the stream rate. Clamp to the ceiling and refuse a requested rate the
/// device's *real* ranges don't cover (a synthetic plughw range is filtered out
/// by the caller before this sees `supported`). `supported` is a list of
/// inclusive (min,max) Hz ranges the hardware genuinely supports.
pub fn choose_stream_rate(
    requested: u32,
    supported: &[(u32, u32)],
) -> Result<u32, AudioError> {
    let want = requested.min(MAX_SAMPLE_RATE);
    if supported.iter().any(|&(lo, hi)| want >= lo && want <= hi) {
        return Ok(want);
    }
    // Fall back to the highest supported rate at or below the ceiling.
    let best = supported
        .iter()
        .map(|&(_, hi)| hi.min(MAX_SAMPLE_RATE))
        .filter(|&r| r > 0)
        .max();
    match best {
        Some(r) => Ok(r),
        None => Err(AudioError::RateTooHigh { requested, ceiling: MAX_SAMPLE_RATE }),
    }
}

/// Pick a capture format from those advertised for `rate`. Never trusts cpal's
/// default; prefers I16. `configs` is a list of (format, min_rate, max_rate).
pub fn pick_input_sample_format(
    configs: &[(SampleFmt, u32, u32)],
    rate: u32,
) -> Option<SampleFmt> {
    configs
        .iter()
        .filter(|&&(_, lo, hi)| rate >= lo && rate <= hi)
        .map(|&(f, _, _)| f)
        .min_by_key(|f| f.rank())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_asound_cards() {
        let sample = " 0 [Device         ]: USB-Audio - USB Audio Device
                      C-Media USB Audio Device at usb-...
 1 [PCH            ]: HDA-Intel - HDA Intel PCH
                      HDA Intel PCH at 0x...";
        let cards = parse_proc_asound_cards(sample);
        assert_eq!(cards, vec![(0, "Device".to_string()), (1, "PCH".to_string())]);
    }

    #[test]
    fn extracts_card_token() {
        assert_eq!(alsa_card_token("plughw:CARD=Device,DEV=0"), Some("Device"));
        assert_eq!(alsa_card_token("plughw:0,0"), None);
    }

    #[test]
    fn rate_is_clamped_to_ceiling() {
        // Device claims up to 192k (synthetic): we never go above 48k.
        assert_eq!(choose_stream_rate(96_000, &[(8_000, 192_000)]).unwrap(), 48_000);
    }

    #[test]
    fn rate_falls_back_to_best_supported_below_ceiling() {
        // Requested 48k unsupported; best real range tops at 44.1k.
        assert_eq!(choose_stream_rate(48_000, &[(8_000, 44_100)]).unwrap(), 44_100);
    }

    #[test]
    fn rate_errors_when_nothing_usable() {
        assert!(matches!(
            choose_stream_rate(48_000, &[]),
            Err(AudioError::RateTooHigh { .. })
        ));
    }

    #[test]
    fn format_prefers_i16_over_f32() {
        let configs = [
            (SampleFmt::F32, 8_000, 48_000),
            (SampleFmt::I16, 8_000, 48_000),
        ];
        assert_eq!(pick_input_sample_format(&configs, 48_000), Some(SampleFmt::I16));
    }

    #[test]
    fn format_respects_rate_window() {
        let configs = [(SampleFmt::I16, 8_000, 16_000), (SampleFmt::F32, 8_000, 48_000)];
        // At 48k only F32 covers the rate.
        assert_eq!(pick_input_sample_format(&configs, 48_000), Some(SampleFmt::F32));
    }
}
