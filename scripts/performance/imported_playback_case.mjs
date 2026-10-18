function finite(value) {
  return typeof value === 'number' && Number.isFinite(value);
}

export function sourceIdentityStatus(snapshot, expectedProvider, expectedRecordingId, previousOperationId = null) {
  const candidate = snapshot?.candidate;
  const operationId = snapshot?.diagnostics?.operationId;
  // A stale or absent candidate while resolving is not evidence of a mismatch.
  if (!finite(snapshot?.diagnostics?.sourceReadyMs)
      || !candidate || typeof candidate.provider !== 'string' || !candidate.provider
      || typeof candidate.id !== 'string' || !candidate.id
      || (previousOperationId && (typeof operationId !== 'string' || operationId === previousOperationId))) {
    return 'pending';
  }
  if (expectedProvider && candidate.provider !== expectedProvider) return 'wrong_provider';
  if (expectedRecordingId && candidate.id !== expectedRecordingId) return 'wrong_source_recording';
  return 'matched';
}

export function assessImportedCase(snapshot, { trackId, previousGeneration, previousOperationId,
  expectedProvider, expectedRecordingId, expectedDurationSeconds }) {
  if (snapshot?.engine !== 'native' || snapshot?.trackId !== trackId
      || !Number.isSafeInteger(snapshot?.generation)
      || snapshot.generation === previousGeneration
      || (typeof previousOperationId === 'string' && snapshot?.diagnostics?.operationId === previousOperationId)) {
    return { status: 'pending' };
  }
  const identity = sourceIdentityStatus(snapshot, expectedProvider, expectedRecordingId);
  if (identity === 'pending') return { status: 'pending' };
  if (identity !== 'matched') return { status: 'failed', failureCategory: identity };

  const queue = Array.isArray(snapshot.queue) ? snapshot.queue : null;
  if (!queue) return { status: 'pending' };
  const queueIds = queue.map((row) => row?.id).filter((value) => typeof value === 'string');
  if (queueIds.length !== 1 || queueIds[0] !== trackId) {
    return { status: 'failed', failureCategory: 'unexpected_isolated_queue' };
  }
  if (!finite(snapshot.duration)) return { status: 'pending' };
  if (Math.abs(snapshot.duration - expectedDurationSeconds) > 1) {
    return { status: 'failed', failureCategory: 'recording_duration_mismatch' };
  }
  return { status: 'ready' };
}


export async function prepareImportedCase({ call, state, sleep, now = Date.now, onMuted = () => {} },
  previous, expected, deadline) {
  if (previous?.engine !== 'native') throw new Error('wrong_engine_or_track');
  const queue = Array.isArray(previous.queue) ? previous.queue : [];
  const ids = queue.map((row) => row?.id).filter((value) => typeof value === 'string');
  if (ids.length !== 1 || ids[0] !== previous.trackId) throw new Error('unexpected_isolated_queue');

  const muted = await call('set_volume', { volume: 0 }, deadline);
  if (muted?.error) throw new Error('volume_setup_failed');
  onMuted();
  const selected = await call('play_tracks', { trackIds: [expected.trackId] }, deadline);
  if (selected?.error || selected?.result?.ok === false) throw new Error('case_selection_failed');

  let latest = null;
  while (now() < deadline) {
    latest = await state(deadline);
    const assessed = assessImportedCase(latest, {
      ...expected,
      previousGeneration: previous.generation,
      previousOperationId: previous.diagnostics?.operationId,
    });
    if (assessed.status === 'ready') return latest;
    if (assessed.status === 'failed') {
      const error = new Error(assessed.failureCategory);
      error.category = assessed.failureCategory;
      error.snapshot = latest;
      throw error;
    }
    await sleep(100);
  }
  const timeout = new Error('case_prepare_timeout');
  timeout.snapshot = latest;
  throw timeout;
}
