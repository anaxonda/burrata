import { createSustainedPlaybackCheck } from './sustained_playback.mjs';
import { sourceIdentityStatus } from './imported_playback_case.mjs';

export function isVerifiedBandcampInput(provider, recordingId, seconds) {
  return provider === 'bandcamp' && /^bandcamp:[1-9][0-9]{0,19}$/.test(recordingId ?? '')
    && Number.isFinite(seconds) && seconds * 0.05 >= 5.25;
}

// Called after the single real prepareImportedCase/play_tracks operation. No replay.
export async function qualifyPreparedStartup(initial, { state, sleep, now = Date.now, deadline,
  trackId, expectedProvider, expectedRecordingId, expectedDurationSeconds }) {
  const checker = createSustainedPlaybackCheck();
  const entryId = initial?.entryId;
  let latest = initial;
  let continuedPlayback = null;
  let failureCategory = null;
  if (typeof entryId !== 'string' || !entryId) failureCategory = 'missing_occurrence_identity';
  while (!failureCategory && now() < deadline) {
    if (latest?.engine !== 'native' || latest.trackId !== trackId || latest.entryId !== entryId
        || latest.generation !== initial.generation || latest.seekSerial !== initial.seekSerial
        || sourceIdentityStatus(latest, expectedProvider, expectedRecordingId) !== 'matched') {
      failureCategory = 'continued_playback_session_changed'; break;
    }
    if (!Number.isFinite(latest.duration) || Math.abs(latest.duration - expectedDurationSeconds) > 1) {
      failureCategory = 'recording_duration_mismatch'; break;
    }
    if (['failed', 'error', 'ended', 'idle', 'stopped'].includes(latest.status)) { failureCategory = 'startup_failed'; break; }
    continuedPlayback = checker.observe(latest, now());
    if (continuedPlayback.status === 'failed') { failureCategory = continuedPlayback.failureCategory; break; }
    if (continuedPlayback.status === 'ready') break;
    await sleep(100);
    latest = await state(deadline);
  }
  if (!failureCategory && continuedPlayback?.status !== 'ready') failureCategory = 'continued_playback_timeout';
  return { type: 'single_prepared_start', index: 1, continuedPlayback, failureCategory, snapshot: latest };
}

export async function runOneShotProtocol(startup, seek) {
  const start = await startup();
  const seeks = [];
  if (!start.failureCategory) {
    for (const fraction of [0.95, 0.01]) {
      const result = await seek(fraction, 1);
      seeks.push(result);
      if (result.failureCategory) break;
    }
  }
  return { start, seeks };
}
