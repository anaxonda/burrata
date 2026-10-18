// Fixed-field telemetry only: never retain paths, URLs, headers or provider payloads.
export function safeBuffering(value) {
  const sample = value?.diagnostics?.lastBuffering;
  const stream = sample?.stream;
  if (!sample || typeof sample !== 'object') return null;
  const count = (field) => Number.isSafeInteger(stream?.[field]) && stream[field] >= 0 ? stream[field] : null;
  const bool = (field) => typeof stream?.[field] === 'boolean' ? stream[field] : null;
  return {
    generation: Number.isSafeInteger(sample.generation) ? sample.generation : null,
    seekSerial: Number.isSafeInteger(sample.seekSerial) ? sample.seekSerial : null,
    stream: stream && typeof stream === 'object' ? {
      readerObserved: bool('readerObserved'), networkReader: bool('networkReader'),
      bytesRead: count('bytesRead'), downloadedRangeEnd: count('downloadedRangeEnd'),
      downloadProgressAgeMs: count('downloadProgressAgeMs'), readInFlightMs: count('readInFlightMs'),
      maxReadMs: count('maxReadMs'), seekInFlightMs: count('seekInFlightMs'), maxSeekMs: count('maxSeekMs'),
      pcmProducedSamples: count('pcmProducedSamples'), pcmConsumedSamples: count('pcmConsumedSamples'),
      syntheticSilenceSamples: count('syntheticSilenceSamples'),
    } : null,
  };
}
