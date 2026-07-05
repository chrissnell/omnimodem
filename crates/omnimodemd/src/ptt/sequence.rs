//! No-sleep TX sequencing. Times PTT off the playback drain watermark, not a
//! fixed sleep. Lifted from Graywolf `tx_worker.rs::drive_tx_cycle`.

use super::{PttDriver, PttError};
use crate::audio::backend::PlaybackHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Outcome of one TX cycle. On any failure PTT has been released (except
/// `KeyFailed`, where the line was never asserted).
#[derive(Debug, PartialEq, Eq)]
pub enum TxCycleOutcome {
    Done,
    /// `cancel` fired mid-burst: the buffered audio was flushed and PTT
    /// released before the samples finished playing (e.g. a mode change).
    Aborted,
    KeyFailed(PttError),
    SubmitFailed(PttError),
    UnkeyFailed(PttError),
}

/// Drive one transmission: key, play `samples`, wait for drain, unkey.
/// `poll` is the drain-loop poll interval (5 ms in production; 0 in tests).
/// `cancel` is polled during the drain wait: when it flips true the sink is
/// flushed and PTT released immediately, returning `Aborted`, so a mode change
/// stops the current transmission instead of letting it play out.
pub fn drive_tx_cycle(
    driver: &mut dyn PttDriver,
    sink: &PlaybackHandle,
    samples: Vec<i16>,
    sample_rate: u32,
    poll: Duration,
    cancel: &AtomicBool,
) -> TxCycleOutcome {
    let n = samples.len();
    let expected = Duration::from_nanos((n as u64 * 1_000_000_000) / sample_rate.max(1) as u64);

    if let Err(e) = driver.key() {
        return TxCycleOutcome::KeyFailed(e);
    }

    let watermark = match sink.submit(samples) {
        Ok(wm) => wm,
        Err(e) => {
            let _ = driver.unkey(); // release before bailing
            return TxCycleOutcome::SubmitFailed(PttError::Io(e.to_string()));
        }
    };

    // Wait until BOTH the DAC drained the watermark AND the expected airtime
    // elapsed. Timeout = expected + 500 ms guards a wedged stream.
    let start = Instant::now();
    let deadline = start + expected + Duration::from_millis(500);
    let mut aborted = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            // Drop the samples the DAC has not played yet so the carrier falls
            // silent now, then unkey below rather than draining the whole burst.
            sink.flush();
            aborted = true;
            break;
        }
        let drained_enough = sink.drained_samples() >= watermark;
        let time_enough = start.elapsed() >= expected;
        if drained_enough && time_enough {
            break;
        }
        if Instant::now() >= deadline {
            break; // proceed to unkey rather than hang forever
        }
        if !poll.is_zero() {
            std::thread::sleep(poll);
        } else {
            std::thread::yield_now();
        }
    }

    match driver.unkey() {
        Ok(()) if aborted => TxCycleOutcome::Aborted,
        Ok(()) => TxCycleOutcome::Done,
        Err(e) => TxCycleOutcome::UnkeyFailed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;
    use crate::ptt::none::MockPtt;

    #[test]
    fn full_cycle_keys_plays_and_unkeys() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let keyed_during = ptt.keyed.clone();

        let out = drive_tx_cycle(&mut ptt, &sink, vec![5i16; 480], 48_000, Duration::ZERO, &AtomicBool::new(false));
        assert_eq!(out, TxCycleOutcome::Done);
        assert!(!keyed_during.load(std::sync::atomic::Ordering::Relaxed), "released after");
        // Audio actually reached the sink.
        assert_eq!(backend.played.lock().unwrap().len(), 480);
    }

    #[test]
    fn key_failure_does_not_submit_or_unkey() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        ptt.fail_key();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![0i16; 10], 48_000, Duration::ZERO, &AtomicBool::new(false));
        assert!(matches!(out, TxCycleOutcome::KeyFailed(_)));
        assert_eq!(backend.played.lock().unwrap().len(), 0, "no audio on key failure");
    }

    #[test]
    fn unkey_failure_is_reported() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        ptt.fail_unkey();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![0i16; 48], 48_000, Duration::ZERO, &AtomicBool::new(false));
        assert!(matches!(out, TxCycleOutcome::UnkeyFailed(_)));
    }

    #[test]
    fn empty_buffer_completes_immediately() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let start = Instant::now();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![], 48_000, Duration::ZERO, &AtomicBool::new(false));
        assert_eq!(out, TxCycleOutcome::Done);
        assert!(start.elapsed() < Duration::from_millis(100), "no spurious sleep");
    }

    // A cancel that is already set when the cycle starts must release PTT and
    // report `Aborted` without waiting out the burst's airtime — this is the
    // path a mode change takes to stop a transmission mid-flight.
    #[test]
    fn cancel_aborts_and_releases_ptt() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let keyed_during = ptt.keyed.clone();
        let cancel = AtomicBool::new(true);

        // A full second of audio would take ~1 s to drain; the abort must return
        // essentially immediately instead.
        let start = Instant::now();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![7i16; 48_000], 48_000, Duration::ZERO, &cancel);
        assert_eq!(out, TxCycleOutcome::Aborted);
        assert!(start.elapsed() < Duration::from_millis(200), "abort must not drain the burst");
        assert!(!keyed_during.load(std::sync::atomic::Ordering::Relaxed), "PTT released on abort");
    }
}
