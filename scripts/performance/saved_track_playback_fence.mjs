// Queue occurrence identity is the playback ownership contract. Player/source
// generations are independent counters and may be absent from WebView snapshots.
export function matchesAcceptedEntry(state, trackId, entryId) {
  return typeof entryId === 'string'
    && state?.entryId === entryId
    && state?.trackId === trackId;
}

export function acceptedEntryOwnershipChanged(state, trackId, entryId) {
  return state?.entryId !== entryId || state?.trackId !== trackId;
}

const safeId = (value) => typeof value === 'string'
  && /^[A-Za-z0-9._:-]{1,120}$/.test(value) ? value : null;

export function playbackSourceIdentity(value) {
  const candidate = value?.candidate ?? {};
  const resolved = value?.lastResolvedSource ?? {};
  return {
    provider: safeId(candidate.provider) ?? safeId(resolved.provider),
    id: safeId(candidate.id) ?? safeId(resolved.id),
  };
}
