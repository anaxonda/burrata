export function webviewMediaProbeConfirmsEngine(playbackState, mediaInspection) {
  const stateEngine = playbackState?.engine;
  return (stateEngine === undefined || stateEngine === 'webview')
    && mediaInspection?.engine === 'webview'
    && mediaInspection?.applicable === true;
}

/**
 * Wait for the app's explicit frontend-readiness response before inspecting media.
 * Only FRONTEND_NOT_READY is transient here; every other inspection error is terminal.
 */
export async function waitForFrontendReady(inspect, {
  timeoutMs = 30_000,
  pollIntervalMs = 250,
  now = () => performance.now(),
  sleep = ms => new Promise(resolve => setTimeout(resolve, ms)),
} = {}) {
  if (typeof inspect !== 'function' || !Number.isFinite(timeoutMs) || timeoutMs <= 0
      || !Number.isFinite(pollIntervalMs) || pollIntervalMs <= 0
      || typeof now !== 'function' || typeof sleep !== 'function') {
    throw new TypeError('Invalid frontend readiness wait options');
  }
  const deadline = now() + timeoutMs;
  let lastNotReady;
  while (now() < deadline) {
    try {
      return await inspect();
    } catch (error) {
      if (error?.code !== 'FRONTEND_NOT_READY') throw error;
      lastNotReady = error;
    }
    const remaining = deadline - now();
    if (remaining <= 0) break;
    await sleep(Math.min(pollIntervalMs, remaining));
  }
  throw lastNotReady ?? Object.assign(new Error('Frontend readiness deadline expired'), {
    code: 'FRONTEND_NOT_READY',
  });
}

function sampleReady(snapshot, playback, trackId) {
  const media = snapshot?.media;
  return playback?.status === 'playing'
    && playback?.trackId === trackId
    && snapshot?.engine === 'webview'
    && snapshot?.applicable === true
    && snapshot?.trackId === trackId
    && snapshot?.queueTrackId === trackId
    && typeof snapshot?.entryId === 'string'
    && snapshot.entryId.length > 0
    && snapshot?.queueTotal === 1
    && snapshot?.previewKind === 'local'
    && media?.present === true
    && media?.sourceStable === true
    && media?.paused === false
    && Number.isInteger(media?.readyState)
    && media.readyState >= 2
    && Number.isFinite(media?.duration)
    && media.duration >= 3.5 && media.duration <= 5.0
    && Number.isFinite(media?.currentTime);
}

/** One attempt: prove startup progress before measuring the steady clock. */
export function createWebviewAudioSmokeGate(trackId, startedAtMs) {
  const startupDeadlineMs = startedAtMs + 10_000;
  let phase = 'startup';
  let initialReady = null, firstProgress = null, first = null, second = null, latest = null;
  let initialReadyAt = null, firstProgressAt = null, firstAt = null, secondAt = null, latestAt = null;
  let previous = null, previousAt = null, validatedSteadyWindows = 0;
  const fail = category => {
    phase = 'failed';
    throw Object.assign(new Error(category), { category });
  };
  const sameIdentity = (left, right) => ['trackId', 'queueTrackId', 'entryId', 'sourceGeneration', 'sessionId', 'seekSerial']
    .every(key => left?.[key] === right?.[key]);
  return {
    get phase() { return phase; },
    startupDeadlineMs,
    observe(playback, inspection, sampledAtMs) {
      if (phase === 'failed' || phase === 'passed') throw new Error('Smoke attempt is already terminal');
      latest = { ...inspection, playbackStatus: playback?.status };
      latestAt = sampledAtMs;
      if (phase === 'steady') { second = latest; secondAt = sampledAtMs; }
      const errorCode = inspection?.media?.errorCode;
      if (errorCode === 4) return fail('unsupported_media');
      if (errorCode === 2) return fail('source_failure');
      if (errorCode === 3) return fail('decode_failure');
      if (playback?.trackId && playback.trackId !== trackId) return fail('wrong_playback_track');
      if (initialReady && !sameIdentity(initialReady, latest)) return fail('playback_identity_changed');
      if (phase === 'startup') {
        if (sampledAtMs >= startupDeadlineMs) return this.expireStartup();
        if (!sampleReady(inspection, playback, trackId)) return phase;
        if (!initialReady) { initialReady = latest; initialReadyAt = sampledAtMs; }
        if (latest.media.currentTime - initialReady.media.currentTime >= 0.5) {
          firstProgress = latest; firstProgressAt = sampledAtMs;
          first = latest; firstAt = sampledAtMs;
          previous = latest; previousAt = sampledAtMs;
          phase = 'steady';
        }
        return phase;
      }
      if (!sampleReady(inspection, playback, trackId)) return fail('audio_state_not_playing');
      if (sampledAtMs - previousAt < 900) return fail('steady_sample_too_early');
      if (second.media.currentTime - previous.media.currentTime < 0.5) return fail('steady_clock_stalled');
      previous = latest; previousAt = sampledAtMs;
      validatedSteadyWindows += 1;
      if (validatedSteadyWindows === 3) phase = 'passed';
      return phase;
    },
    expireStartup() {
      if (phase !== 'startup') throw new Error('Startup attempt is already terminal');
      return fail(initialReady ? 'audio_clock_not_started' : 'audio_not_ready');
    },
    diagnostics(nowMs) {
      const elapsed = time => time === null ? null : time - startedAtMs;
      return { initialReady, firstProgress, first, second, latest, sampling: {
        initialReadyElapsedMs: elapsed(initialReadyAt), firstProgressElapsedMs: elapsed(firstProgressAt),
        firstSampleElapsedMs: elapsed(firstAt), secondSampleElapsedMs: elapsed(secondAt), latestSampleElapsedMs: elapsed(latestAt),
        betweenSamplesWallMs: firstAt === null || secondAt === null ? null : secondAt - firstAt,
        totalElapsedMs: nowMs - startedAtMs,
        validatedSteadyWindows,
      } };
    },
  };
}
