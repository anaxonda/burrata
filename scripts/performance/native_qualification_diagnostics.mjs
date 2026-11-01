// Fixed-field telemetry only: never retain paths, URLs, headers or provider payloads.
export function safeBuffering(value) {
  const sample = value?.diagnostics?.lastBuffering ?? value?.lastBuffering;
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


// Project diagnostic observations only. A progress clock or decoder readiness
// event is never upgraded into proof of EOF, complete PCM, or audible output.
export function safeNativeTimeline(report) {
  const rows=report?.playback?.nativePlayback ?? report?.nativePlayback;
  if(!Array.isArray(rows))return [];
  const count=v=>Number.isSafeInteger(v)&&v>=0?v:null;
  const seconds=v=>Number.isFinite(v)&&v>=0&&v<=86400?v:null;
  const uuid=v=>typeof v==='string'&&/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(v)?v:null;
  const moments=['snapshot','progress','clean_eof','premature_eof','output_ended'];
  const statuses=['idle','resolving','playing','paused','buffering','ended','failed','stopped'];
  return rows.slice(-256).filter(row=>row?.playback?.engine==='native'&&moments.includes(row.moment)).map(row=>({
    sequence:count(row.sequence),atUnixMs:count(row.atUnixMs),elapsedMs:count(row.elapsedMs),moment:row.moment,
    generation:count(row.playback.generation),seekSerial:count(row.playback.seekSerial),entryId:uuid(row.playback.entryId),
    status:statuses.includes(row.playback.status)?row.playback.status:null,
    position:seconds(row.playback.position),duration:seconds(row.playback.duration),catalogDurationSeconds:seconds(row.playback.catalogDurationSeconds),
    lastBuffering:safeBuffering(row.playback),
  }));
}
