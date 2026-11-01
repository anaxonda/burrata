import test from 'node:test';
import assert from 'node:assert/strict';
import { safeBuffering, safeNativeTimeline } from './native_qualification_diagnostics.mjs';

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


test('Native export timeline keeps explicit EOF distinct from decoder readiness and advancing clock',()=>{
  const base={engine:'native',generation:3,seekSerial:1,entryId:'550e8400-e29b-41d4-a716-446655440000',status:'playing',position:12,duration:24,catalogDurationSeconds:12,url:'private'};
  const actual=safeNativeTimeline({playback:{nativePlayback:[
    {moment:'decoder_end',playback:base},
    {sequence:1,moment:'progress',atUnixMs:100,elapsedMs:0,playback:base},
    {sequence:2,moment:'clean_eof',atUnixMs:200,elapsedMs:100,playback:{...base,duration:12}},
  ]}});
  assert.equal(actual.length,2);assert.equal(actual[0].moment,'progress');assert.equal(actual[1].moment,'clean_eof');
  assert.equal(actual[0].duration,24);assert.equal(actual[1].duration,12);assert.equal(JSON.stringify(actual).includes('private'),false);
});

test('Native timeline reader bounds rows, unknown values and does not fabricate zero counters',()=>{
  assert.deepEqual(safeNativeTimeline({}),[]);
  const rows=Array.from({length:300},()=>({moment:'snapshot',playback:{engine:'native',generation:-1,position:Infinity,duration:-1,entryId:'private',status:'private'}}));
  const result=safeNativeTimeline({nativePlayback:rows});assert.equal(result.length,256);
  assert.equal(result[0].generation,null);assert.equal(result[0].position,null);assert.equal(result[0].duration,null);assert.equal(result[0].entryId,null);assert.equal(result[0].status,null);
  assert.equal(result[0].lastBuffering,null);
});

test('Native exported last-buffering facts use the same fixed projection',()=>{
  assert.equal(safeBuffering({lastBuffering:{generation:4,seekSerial:1,stream:{pcmConsumedSamples:2048,token:'private'}}}).stream.pcmConsumedSamples,2048);
});
