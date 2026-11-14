//! Idle-only policy for optional provider work, independent of output volume.
use std::time::{Duration, Instant};

pub(crate) const QUIET_PERIOD: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct BackgroundWorkPolicy {
    playback_active: bool,
    last_foreground: Option<Instant>,
}

impl BackgroundWorkPolicy {
    pub(crate) fn foreground(&mut self, now: Instant) {
        self.last_foreground = Some(now);
    }

    /// Returns true when entering active playback, so callers can cancel work
    /// under the same lock used to register it. Repeated position publications
    /// must neither repeat cancellation nor move the idle deadline.
    pub(crate) fn observe_playback(&mut self, status: &str, now: Instant) -> bool {
        let active = matches!(status, "resolving" | "buffering" | "playing");
        let entered = active && !self.playback_active;
        if active != self.playback_active {
            self.foreground(now);
            self.playback_active = active;
        }
        entered
    }

    pub(crate) fn quiet_remaining(&self, now: Instant) -> Duration {
        if self.playback_active {
            return QUIET_PERIOD;
        }
        self.last_foreground
            .map(|last| QUIET_PERIOD.saturating_sub(now.saturating_duration_since(last)))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_playback_never_becomes_idle_after_the_old_timeout() {
        for status in ["resolving", "buffering", "playing"] {
            let mut policy = BackgroundWorkPolicy::default();
            let now = Instant::now();
            assert!(policy.observe_playback(status, now));
            assert!(!policy
                .quiet_remaining(now + Duration::from_secs(3600))
                .is_zero());
        }
    }

    #[test]
    fn idle_grace_starts_at_pause_and_is_not_extended_by_position_updates() {
        let mut policy = BackgroundWorkPolicy::default();
        let now = Instant::now();
        policy.observe_playback("playing", now);
        let paused = now + Duration::from_secs(100);
        assert!(!policy.observe_playback("paused", paused));
        assert_eq!(policy.quiet_remaining(paused), QUIET_PERIOD);
        policy.observe_playback("paused", paused + Duration::from_secs(29));
        assert_eq!(
            policy.quiet_remaining(paused + Duration::from_secs(29)),
            Duration::from_secs(1)
        );
        assert!(policy.quiet_remaining(paused + QUIET_PERIOD).is_zero());
    }

    #[test]
    fn resume_cancels_again_and_loading_transitions_keep_work_blocked() {
        let mut policy = BackgroundWorkPolicy::default();
        let now = Instant::now();
        assert!(policy.observe_playback("resolving", now));
        for (offset, status) in [(2, "buffering"), (3, "playing"), (60, "buffering")] {
            assert!(!policy.observe_playback(status, now + Duration::from_secs(offset)));
            assert!(!policy
                .quiet_remaining(now + Duration::from_secs(offset))
                .is_zero());
        }
        policy.observe_playback("paused", now + Duration::from_secs(61));
        assert!(policy.observe_playback("playing", now + Duration::from_secs(62)));
    }

    #[test]
    fn terminal_states_release_work_after_a_quiet_period() {
        for status in ["ended", "failed", "idle"] {
            let mut policy = BackgroundWorkPolicy::default();
            let now = Instant::now();
            policy.observe_playback("playing", now);
            policy.observe_playback(status, now + Duration::from_secs(1));
            assert!(policy
                .quiet_remaining(now + Duration::from_secs(31))
                .is_zero());
        }
    }

    #[test]
    fn foreground_request_still_defers_work_before_playback_status_arrives() {
        let mut policy = BackgroundWorkPolicy::default();
        let now = Instant::now();
        assert!(policy.quiet_remaining(now).is_zero());
        policy.foreground(now);
        assert_eq!(policy.quiet_remaining(now), QUIET_PERIOD);
    }
}
