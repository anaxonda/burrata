// Native positions come from output consumption; firstAudioMs records the first
// mixer-consumed frames. Require both, a stable session and five measured seconds.
export function createSustainedPlaybackCheck(requiredMs = 5000) {
  let first = null, previous = null, previousChanged = null, failure = null, observations = 0;
  return {
    observe(snapshot, atMs, audioField = 'firstAudioMs') {
      if (failure) return { status: 'failed', failureCategory: failure };
      const valid = snapshot?.engine === 'native' && snapshot?.status === 'playing'
        && Number.isSafeInteger(snapshot?.generation) && Number.isSafeInteger(snapshot?.seekSerial)
        && typeof snapshot?.diagnostics?.operationId === 'string' && snapshot.diagnostics.operationId.length > 0
        && Number.isFinite(snapshot?.position) && snapshot.position >= 0
        && Number.isFinite(snapshot?.diagnostics?.[audioField]) && snapshot.diagnostics[audioField] >= 0;
      if (!valid) {
        if (first) failure = 'continued_playback_interrupted';
        return { status: failure ? 'failed' : 'pending', ...(failure ? { failureCategory: failure } : {}) };
      }
      const identity = JSON.stringify([snapshot.engine, snapshot.trackId, snapshot.generation,
        snapshot.seekSerial, snapshot.diagnostics.operationId, snapshot.diagnostics.seekOperationId,
        snapshot.candidate?.provider, snapshot.candidate?.id]);
      const sample = { identity, atMs, position: snapshot.position, buffering: snapshot.diagnostics.bufferingCount, recovery: snapshot.diagnostics.recoveryCount };
      if (previous) {
        const wall = atMs - previous.atMs, audio = (sample.position - previous.position) * 1000;
        if ((Number.isFinite(sample.buffering) && Number.isFinite(previous.buffering) && sample.buffering > previous.buffering)
            || (Number.isFinite(sample.recovery) && Number.isFinite(previous.recovery) && sample.recovery > previous.recovery)) failure = 'continued_playback_interrupted';
        else if (identity !== previous.identity) failure = 'continued_playback_session_changed';
        else if (wall <= 0 || wall > 2000) failure = 'continued_playback_observation_gap';
        // native_output publishes mixer consumption on a >=250ms worker
        // cadence (with a 40ms idle receive wait). Faster polls repeat the old
        // position; those held snapshots must not reset the progress clock.
        // Keep per-poll status/identity/counter/gap and backward checks above.
        else if (audio < -10 || (audio > 0 && (sample.position - previousChanged.position) * 1000
            > (atMs - previousChanged.atMs) * 1.25 + 250)) failure = 'continued_playback_position_jump';
      }
      if (failure) return { status: 'failed', failureCategory: failure };
      first ??= sample; previous = sample;
      if (!previousChanged || sample.position > previousChanged.position) previousChanged = sample;
      observations++;
      const wallMs = atMs - first.atMs, advancedMs = (sample.position - first.position) * 1000;
      return { status: wallMs >= requiredMs && advancedMs >= requiredMs ? 'ready' : 'pending',
        wallMs, advancedMs, observations };
    },
  };
}
