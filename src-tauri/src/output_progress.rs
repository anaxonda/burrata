//! Monitor actual device consumption independently of audio amplitude.
use std::time::Instant;

/// The worker may outpace command handling. Keep one latest position and one
/// notification, while terminal events retain their ordered delivery.
#[derive(Default)]
pub(super) struct LatestProgress(std::sync::Mutex<Option<(u64, f64)>>);
impl LatestProgress {
    pub(super) fn update(&self, generation: u64, seconds: f64) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace((generation, seconds))
            .is_none()
    }
    pub(super) fn take(&self) -> Option<(u64, f64)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

#[derive(Default)]
pub(super) struct OutputProgressWatch {
    pub(super) observed: Option<(u64, u64, Instant)>,
}
impl OutputProgressWatch {
    pub(super) fn stalled(
        &mut self,
        generation: u64,
        samples: u64,
        eligible: bool,
        now: Instant,
    ) -> bool {
        if !eligible {
            self.observed = None;
            return false;
        }
        match self.observed {
            Some((owner, previous, since)) if owner == generation && previous == samples => {
                now.saturating_duration_since(since)
                    >= crate::playback_policy::OUTPUT_PROGRESS_TIMEOUT
            }
            _ => {
                self.observed = Some((generation, samples, now));
                false
            }
        }
    }
}

#[cfg(test)]
mod progress_watch_tests {
    use super::*;
    #[test]
    fn slow_consumers_receive_only_the_latest_position_and_keep_generation() {
        let progress = LatestProgress::default();
        assert!(progress.update(1, 0.0));
        for position in 1..10_000 {
            assert!(!progress.update(2, position as f64));
        }
        assert_eq!(progress.take(), Some((2, 9999.0)));
        assert_eq!(progress.take(), None);
        assert!(progress.update(3, 0.0));
    }
    #[test]
    fn stopped_callback_recovers_but_pause_buffering_and_new_occurrences_do_not() {
        let now = Instant::now();
        let later = now + crate::playback_policy::OUTPUT_PROGRESS_TIMEOUT;
        let mut watch = OutputProgressWatch::default();
        assert!(!watch.stalled(1, 10, true, now));
        assert!(watch.stalled(1, 10, true, later));
        assert!(!watch.stalled(1, 11, true, later));
        assert!(!watch.stalled(2, 11, true, later));
        assert!(!watch.stalled(2, 11, false, later));
        assert!(!watch.stalled(2, 11, true, later));
        assert!(!watch.stalled(2, 0, true, later));
        assert!(watch.stalled(
            2,
            0,
            true,
            later + crate::playback_policy::OUTPUT_PROGRESS_TIMEOUT
        ));
    }
}
