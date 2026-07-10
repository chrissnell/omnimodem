//! The no-op driver (VOX / none) and the test double.

use super::{PttDriver, PttError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// No hardware line: audio alone triggers TX (VOX), or PTT is disabled.
pub struct NonePtt;

impl PttDriver for NonePtt {
    fn key(&mut self) -> Result<(), PttError> {
        Ok(())
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        Ok(())
    }
}

/// Records key/unkey for tests and asserts release-on-drop. Shared counters so
/// a test can inspect state after the driver is handed to a worker/dropped.
#[derive(Clone, Default)]
pub struct MockPtt {
    pub keyed: Arc<AtomicBool>,
    pub keys: Arc<AtomicUsize>,
    pub unkeys: Arc<AtomicUsize>,
    fail_key: Arc<AtomicBool>,
    fail_unkey: Arc<AtomicBool>,
}

impl MockPtt {
    pub fn new() -> Self {
        MockPtt::default()
    }
    pub fn fail_key(&self) {
        self.fail_key.store(true, Ordering::Relaxed);
    }
    pub fn fail_unkey(&self) {
        self.fail_unkey.store(true, Ordering::Relaxed);
    }
    pub fn is_keyed(&self) -> bool {
        self.keyed.load(Ordering::Relaxed)
    }
}

impl PttDriver for MockPtt {
    fn key(&mut self) -> Result<(), PttError> {
        if self.fail_key.load(Ordering::Relaxed) {
            return Err(PttError::Io("mock key failure".into()));
        }
        self.keys.fetch_add(1, Ordering::Relaxed);
        self.keyed.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        if self.fail_unkey.load(Ordering::Relaxed) {
            return Err(PttError::Io("mock unkey failure".into()));
        }
        self.unkeys.fetch_add(1, Ordering::Relaxed);
        self.keyed.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// Release PTT if the driver is dropped while keyed (panic/shutdown safety).
impl Drop for MockPtt {
    fn drop(&mut self) {
        if self.is_keyed() {
            let _ = self.unkey();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_a_noop() {
        let mut p = NonePtt;
        assert!(p.key().is_ok());
        assert!(p.unkey().is_ok());
    }

    #[test]
    fn mock_records_key_and_unkey() {
        let mut p = MockPtt::new();
        p.key().unwrap();
        assert!(p.is_keyed());
        p.unkey().unwrap();
        assert!(!p.is_keyed());
        assert_eq!(p.keys.load(Ordering::Relaxed), 1);
        assert_eq!(p.unkeys.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn mock_can_inject_failures() {
        let mut p = MockPtt::new();
        p.fail_key();
        assert!(matches!(p.key(), Err(PttError::Io(_))));
    }

    #[test]
    fn drop_while_keyed_unkeys() {
        let probe = MockPtt::new();
        let unkeys = probe.unkeys.clone();
        {
            let mut p = probe.clone();
            p.key().unwrap();
            // p drops here while keyed
        }
        assert_eq!(unkeys.load(Ordering::Relaxed), 1, "drop must release PTT");
    }
}
