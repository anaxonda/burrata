import test from 'node:test';
import assert from 'node:assert/strict';
import { assessImportedCase, prepareImportedCase, sourceIdentityStatus } from './imported_playback_case.mjs';

const expected = {
  trackId: 'spotify:catalog-a', previousGeneration: 4, previousOperationId: 'operation-old',
  expectedProvider: 'monochrome', expectedRecordingId: 'monochrome:source-a',
  expectedDurationSeconds: 245.48,
};
const snapshot = (overrides = {}) => ({
  engine: 'native', trackId: expected.trackId, generation: 5,
  status: 'playing', duration: 245.48,
  queue: [{ id: expected.trackId }],
  candidate: { provider: expected.expectedProvider, id: expected.expectedRecordingId },
  diagnostics: { sourceReadyMs: 120, operationId: 'operation-new' },
  ...overrides,
});

test('transient missing candidate remains pending while source resolves', () => {
  assert.equal(sourceIdentityStatus(snapshot({ candidate: null, diagnostics: {} }),
    expected.expectedProvider, expected.expectedRecordingId, expected.previousOperationId), 'pending');
  assert.deepEqual(assessImportedCase(snapshot({ candidate: null, diagnostics: {} }), expected), { status: 'pending' });
});

test('a stale candidate before sourceReadyMs is not treated as a mismatch', () => {
  const pending = snapshot({ candidate: { provider: 'youtube', id: 'youtube:stale' },
    diagnostics: { operationId: 'operation-old', sourceReadyMs: 70 } });
  assert.equal(sourceIdentityStatus(pending, expected.expectedProvider, expected.expectedRecordingId,
    expected.previousOperationId), 'pending');
});

test('resolved wrong provider and source recording remain explicit failures', () => {
  assert.deepEqual(assessImportedCase(snapshot({ candidate: { provider: 'youtube', id: 'youtube:source-a' } }), expected),
    { status: 'failed', failureCategory: 'wrong_provider' });
  assert.deepEqual(assessImportedCase(snapshot({ candidate: { provider: 'monochrome', id: 'monochrome:wrong' } }), expected),
    { status: 'failed', failureCategory: 'wrong_source_recording' });
});

test('setup requires a fresh generation and exactly one selected queue item', () => {
  assert.deepEqual(assessImportedCase(snapshot({ generation: 4 }), expected), { status: 'pending' });
  assert.deepEqual(assessImportedCase(snapshot({ queue: [{ id: expected.trackId }, { id: 'other' }] }), expected),
    { status: 'failed', failureCategory: 'unexpected_isolated_queue' });
});

test('a resolved expected source, duration, and isolated queue are ready', () => {
  assert.deepEqual(assessImportedCase(snapshot(), expected), { status: 'ready' });
});

test('case setup mutes before selecting and waits through unresolved source identity', async () => {
  const calls = [];
  let index = 0;
  let clock = 0;
  const states = [
    snapshot({ candidate: { provider: 'youtube', id: 'youtube:old' }, diagnostics: { sourceReadyMs: 50, operationId: 'operation-old' } }),
    snapshot(),
  ];
  const ready = await prepareImportedCase({
    call: async (name, args) => { calls.push([name, args]); return { result: { ok: true } }; },
    state: async () => states[index++], sleep: async (ms) => { clock += ms; },
    now: () => clock,
  }, { engine: 'native', trackId: 'spotify:prior', generation: 4,
    diagnostics: { operationId: 'operation-old' }, queue: [{ id: 'spotify:prior' }] },
  expected, 1000);
  assert.deepEqual(calls.map(([name]) => name), ['set_volume', 'play_tracks']);
  assert.equal(calls[0][1].volume, 0);
  assert.deepEqual(calls[1][1].trackIds, [expected.trackId]);
  assert.equal(ready.candidate.id, expected.expectedRecordingId);
});

test('case setup rejects a resolved incorrect source and retains its snapshot', async () => {
  const calls = [];
  const wrong = snapshot({ candidate: { provider: 'monochrome', id: 'monochrome:wrong' } });
  await assert.rejects(prepareImportedCase({
    call: async (name) => { calls.push(name); return { result: { ok: true } }; },
    state: async () => wrong, sleep: async () => {}, now: () => 0,
  }, { engine: 'native', trackId: 'spotify:prior', generation: 4,
    diagnostics: { operationId: 'operation-old' }, queue: [{ id: 'spotify:prior' }] },
  expected, 1000), (error) => error.category === 'wrong_source_recording' && error.snapshot === wrong);
  assert.deepEqual(calls, ['set_volume', 'play_tracks']);
});

test('case setup refuses a non-single-track starting queue before playback commands', async () => {
  const calls = [];
  await assert.rejects(prepareImportedCase({
    call: async (name) => { calls.push(name); return { result: { ok: true } }; },
    state: async () => { throw new Error('must_not_poll'); }, sleep: async () => {}, now: () => 0,
  }, { engine: 'native', trackId: 'a', generation: 4, diagnostics: { operationId: 'operation-old' },
    queue: [{ id: 'a' }, { id: 'b' }] }, expected, 1000), /unexpected_isolated_queue/);
  assert.deepEqual(calls, []);
});
