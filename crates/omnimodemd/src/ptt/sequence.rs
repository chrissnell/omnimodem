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

/// Sleep for `dur`, polling `cancel` so a mode change interrupts the wait
/// promptly. Returns `true` if `cancel` tripped before `dur` elapsed (caller
/// should abort), `false` if the full duration was served. A zero duration is
/// a no-op that still reports the current cancel state.
fn sleep_cancellable(dur: Duration, poll: Duration, cancel: &AtomicBool) -> bool {
    if cancel.load(Ordering::Relaxed) {
        return true;
    }
    if dur.is_zero() {
        return false;
    }
    // Cap the per-iteration wait so cancel latency stays bounded even when the
    // caller passes poll=0 (tests) or a coarse poll.
    let step = poll.clamp(Duration::from_millis(1), Duration::from_millis(5));
    let deadline = Instant::now() + dur;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        std::thread::sleep(step.min(deadline - now));
    }
}

/// Drive one transmission: key, hold `tx_delay` keyed-but-silent, play
/// `samples`, wait for drain, hold `tx_tail` keyed-but-silent, unkey.
/// `poll` is the drain-loop poll interval (5 ms in production; 0 in tests).
/// `cancel` is polled throughout: when it flips true the sink is flushed and
/// PTT released promptly, returning `Aborted`, so a mode change stops the
/// current transmission instead of letting it play out. `tx_delay` is the
/// per-channel PTT keying lead-in (rig settle before audio) and `tx_tail` the
/// hold after audio drains before releasing; both are interruptible by cancel.
#[allow(clippy::too_many_arguments)]
pub fn drive_tx_cycle(
    driver: &mut dyn PttDriver,
    sink: &PlaybackHandle,
    samples: Vec<i16>,
    sample_rate: u32,
    poll: Duration,
    cancel: &AtomicBool,
    tx_delay: Duration,
    tx_tail: Duration,
) -> TxCycleOutcome {
    let n = samples.len();
    let expected = Duration::from_nanos((n as u64 * 1_000_000_000) / sample_rate.max(1) as u64);

    if let Err(e) = driver.key() {
        return TxCycleOutcome::KeyFailed(e);
    }

    // Keyed-but-silent lead-in: give the rig's PTT time to close before audio.
    // A cancel here (mode change during lead-in) releases without ever playing.
    if sleep_cancellable(tx_delay, poll, cancel) {
        return match driver.unkey() {
            Ok(()) => TxCycleOutcome::Aborted,
            Err(e) => TxCycleOutcome::UnkeyFailed(e),
        };
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

    // Keyed-but-silent tail: hold the key after audio drains (skip on abort —
    // a mode change wants the carrier down now, not after the tail).
    if !aborted {
        sleep_cancellable(tx_tail, poll, cancel);
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

        let out = drive_tx_cycle(&mut ptt, &sink, vec![5i16; 480], 48_000, Duration::ZERO, &AtomicBool::new(false), Duration::ZERO, Duration::ZERO);
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
        let out = drive_tx_cycle(&mut ptt, &sink, vec![0i16; 10], 48_000, Duration::ZERO, &AtomicBool::new(false), Duration::ZERO, Duration::ZERO);
        assert!(matches!(out, TxCycleOutcome::KeyFailed(_)));
        assert_eq!(backend.played.lock().unwrap().len(), 0, "no audio on key failure");
    }

    #[test]
    fn unkey_failure_is_reported() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        ptt.fail_unkey();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![0i16; 48], 48_000, Duration::ZERO, &AtomicBool::new(false), Duration::ZERO, Duration::ZERO);
        assert!(matches!(out, TxCycleOutcome::UnkeyFailed(_)));
    }

    #[test]
    fn empty_buffer_completes_immediately() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let start = Instant::now();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![], 48_000, Duration::ZERO, &AtomicBool::new(false), Duration::ZERO, Duration::ZERO);
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
        let out = drive_tx_cycle(&mut ptt, &sink, vec![7i16; 48_000], 48_000, Duration::ZERO, &cancel, Duration::ZERO, Duration::ZERO);
        assert_eq!(out, TxCycleOutcome::Aborted);
        assert!(start.elapsed() < Duration::from_millis(200), "abort must not drain the burst");
        assert!(!keyed_during.load(std::sync::atomic::Ordering::Relaxed), "PTT released on abort");
    }

    // A nonzero TX delay + tail must extend the keyed window (lead-in before
    // audio, hold after drain) while still completing cleanly and releasing.
    #[test]
    fn delay_and_tail_extend_the_keyed_window() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let keyed_during = ptt.keyed.clone();

        let start = Instant::now();
        // 480 samples == 10 ms audio; 40 ms delay + 40 ms tail dominate.
        let out = drive_tx_cycle(
            &mut ptt, &sink, vec![5i16; 480], 48_000, Duration::from_millis(5),
            &AtomicBool::new(false), Duration::from_millis(40), Duration::from_millis(40),
        );
        assert_eq!(out, TxCycleOutcome::Done);
        assert!(start.elapsed() >= Duration::from_millis(80), "delay+tail must be served");
        assert!(!keyed_during.load(std::sync::atomic::Ordering::Relaxed), "released after tail");
        assert_eq!(backend.played.lock().unwrap().len(), 480, "audio still played");
    }

    // A cancel that trips during the keyed lead-in must release PTT and report
    // `Aborted` before any audio is ever submitted to the sink.
    #[test]
    fn cancel_during_lead_in_aborts_before_audio() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let keyed_during = ptt.keyed.clone();
        let cancel = AtomicBool::new(true); // already set: abort during lead-in

        let out = drive_tx_cycle(
            &mut ptt, &sink, vec![9i16; 480], 48_000, Duration::ZERO,
            &cancel, Duration::from_millis(500), Duration::ZERO,
        );
        assert_eq!(out, TxCycleOutcome::Aborted);
        assert_eq!(backend.played.lock().unwrap().len(), 0, "no audio on lead-in abort");
        assert!(!keyed_during.load(std::sync::atomic::Ordering::Relaxed), "PTT released on abort");
    }
}
