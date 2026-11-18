//! Shared timing policy. Startup remains bounded independently of recovery of
//! an occurrence that has already delivered audio.
use std::time::Duration;

pub(crate) fn position_duration(seconds: f64) -> Result<Duration, String> {
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| "Playback position is outside the supported time range".to_owned())
}

pub(crate) const BUFFERING_GRACE: Duration = Duration::from_secs(15);
pub(crate) const NETWORK_READ_TIMEOUT_US: &str = "20000000";
pub(crate) const RECOVERY_BUDGET: Duration = Duration::from_secs(30);
pub(crate) const OUTPUT_PROGRESS_TIMEOUT: Duration = Duration::from_secs(10);
// Three seconds of stereo PCM at 44.1kHz, filled after immediate first audio.
pub(crate) const PCM_BLOCK_CAPACITY: usize = 64;
pub(crate) const MAX_RECOVERY_ATTEMPTS: u8 = 2;
pub(crate) const INCOMPLETE_HTTP_AUDIO: &str =
    "Native HTTP audio stream ended before the recording finished";

pub(crate) fn chapter_ended_early(position: f64, end: f64) -> bool {
    end.is_finite() && end - position > 0.25
}

pub(crate) fn recording_ended_early(position: f64, duration: f64) -> bool {
    duration.is_finite() && duration - position > (duration * 0.05).clamp(3.0, 10.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryStep {
    RefreshSameSource,
    VerifiedAlternative,
    Exhausted,
}

pub(crate) fn recovery_step(attempt: u8, alternatives_allowed: bool) -> RecoveryStep {
    match attempt {
        1 => RecoveryStep::RefreshSameSource,
        2 if alternatives_allowed => RecoveryStep::VerifiedAlternative,
        2 => RecoveryStep::RefreshSameSource,
        _ => RecoveryStep::Exhausted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positions_must_fit_the_decoder_clock_without_capping_catalog_estimates() {
        for value in [f64::NAN, f64::INFINITY, -1.0, 1e300] {
            assert!(position_duration(value).is_err());
        }
        assert_eq!(
            position_duration(30.25).unwrap(),
            Duration::from_millis(30250)
        );
        assert!(position_duration(86_400.).is_ok());
    }
    #[test]
    fn recovery_refreshes_then_uses_verified_alternatives_with_a_finite_end() {
        assert_eq!(recovery_step(1, true), RecoveryStep::RefreshSameSource);
        assert_eq!(recovery_step(2, true), RecoveryStep::VerifiedAlternative);
        assert_eq!(recovery_step(2, false), RecoveryStep::RefreshSameSource);
        for allowed in [true, false] {
            assert_eq!(recovery_step(3, allowed), RecoveryStep::Exhausted);
            assert_eq!(recovery_step(u8::MAX, allowed), RecoveryStep::Exhausted);
        }
    }
}
