import test from 'node:test';
import assert from 'node:assert/strict';
import { safeBuffering } from './native_qualification_diagnostics.mjs';

test('retains starvation facts and drops all unapproved provider data', () => {
  const facts = { readerObserved: true, networkReader: true, bytesRead: 0,
    downloadedRangeEnd: 456, downloadProgressAgeMs: 70, readInFlightMs: 3,
    maxReadMs: 9, seekInFlightMs: null, maxSeekMs: 7, pcmProducedSamples: 4096,
    pcmConsumedSamples: 2048, syntheticSilenceSamples: 1024 };
  const actual = safeBuffering({ diagnostics: { lastBuffering: { generation: 8,
    seekSerial: 3, secret: 'private', stream: { ...facts, url: 'private', path: 'private', token: 'private' } } } });
  assert.deepEqual(actual, { generation: 8, seekSerial: 3, stream: facts });
  assert.equal(JSON.stringify(actual).includes('private'), false);
});

test('missing and invalid measurements stay unknown rather than becoming zero', () => {
  assert.equal(safeBuffering({}), null);
  const actual = safeBuffering({ diagnostics: { lastBuffering: { stream: {
    readerObserved: 'true', networkReader: false, bytesRead: -1, maxReadMs: Infinity,
    downloadedRangeEnd: '456', pcmProducedSamples: 1.5 } } } });
  assert.equal(actual.generation, null);
  assert.equal(actual.stream.networkReader, false);
  for (const key of ['readerObserved', 'bytesRead', 'maxReadMs', 'downloadedRangeEnd', 'pcmProducedSamples'])
    assert.equal(actual.stream[key], null);
});
