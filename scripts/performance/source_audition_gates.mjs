// Both authenticated projections omit filesystem paths. Compare real recording
// metadata here; the Python gate checks the disposable persisted queue's path.
export function auditionQueueMetadataRetained(queued, saved) {
  if (!queued || !saved || typeof queued !== 'object' || typeof saved !== 'object') return false;
  const required = ['id', 'provider', 'providerId', 'title', 'artist'];
  if (!required.every(key => typeof saved[key] === 'string' && saved[key].trim() && queued[key] === saved[key])) return false;
  return ['album', 'version', 'isrc', 'assetId', 'localAvailability', 'durationSeconds']
    .every(key => JSON.stringify(queued[key]) === JSON.stringify(saved[key]));
}
