import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, symlink, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { observeTransition, requireIsolatedManifest, safeSample } from './next_preparation_observer.mjs';

const spec = { localTrackId: 'local:control', nextTrackId: 'youtube:abcdefghijk', nextVideoId: 'abcdefghijk', nextDurationSeconds: 228 };

test('native contract fixture stays tied to Rust serialization and MCP pagination fields', async () => {
  const fixture = JSON.parse(await readFile(new URL('./next_preparation_native_contract.json', import.meta.url), 'utf8'));
  for (const source of fixture.sources) {
    const text = await readFile(new URL(`../../${source.path}`, import.meta.url), 'utf8');
    for (const snippet of source.requiredSnippets) assert.ok(text.includes(snippet), `${source.path}: ${snippet}`);
  }
  const sample = safeSample(fixture.snapshot, 60_000);
  assert.equal(sample.operationFirstAudioMs, 4); assert.equal(sample.generation, 3);
  assert.equal(sample.sourceProvider, 'youtube'); assert.equal(sample.sourceVideoId, spec.nextVideoId);
  assert.equal(sample.selectedFormatId, '251');
  assert.equal(fixture.snapshot.queueOffset, 0); assert.equal(fixture.snapshot.queueLimit, 2);
  assert.equal(fixture.snapshot.queueHasMore, false);
});
async function trial(change = value => value) {
  let time = 0, selected = false, sought = false;
  const calls = [];
  const value = () => {
    if (!selected) return { engine: 'native', status: 'idle' };
    const next = sought && time >= 60_000;
    return change({ engine: 'native', status: 'playing', queueTotal: 2, queueOffset: 0, queueLimit: 2, queueHasMore: false,
      queue: [{ id: spec.localTrackId }, { id: spec.nextTrackId }], queueEntryIds: ['local-occurrence', 'next-occurrence'],
      trackId: next ? spec.nextTrackId : spec.localTrackId, entryId: next ? 'next-occurrence' : 'local-occurrence',
      generation: next ? 3 : sought ? 2 : 1, seekSerial: sought ? 1 : 0,
      position: next ? (time-60_000)/1000+.1 : sought ? 30+time/1000 : .2,
      duration: next ? 228 : 90, quality: next ? { formatId: '251' } : {},
      candidate: next ? { provider: 'youtube', id: spec.nextTrackId } : null,
      diagnostics: { operationId: next ? 'transition-operation' : 'control-operation',
        operationTrace: [{ phase: 'first_audio', atMs: 0 }], recoveryCount: 0, bufferingCount: 0 },
    }, time);
  };
  const result = await observeTransition({ now: () => time, sleep: async ms => { time += ms; },
    call: async (name, args) => {
      calls.push([name, args]);
      if (name === 'play_tracks') selected = true;
      if (name === 'seek_playback') sought = true;
      return name === 'playback_state' ? value() : {};
    } }, spec);
  return { result, calls };
}

test('one queue and one initial seek lead to a natural next occurrence and five advancing seconds', async () => {
  const { result, calls } = await trial();
  assert.equal(result.transitionOk, true); assert.equal(result.originalFailure, null);
  assert.ok(result.sustainedNextSeconds >= 4.9); assert.equal(result.observedHandoffDelayMs, 0);
  assert.equal(result.networkWireBytes, null); assert.equal(result.physicalAudibilityMeasured, false);
  assert.deepEqual(calls.filter(([name]) => name !== 'playback_state'), [
    ['set_volume', { volume: 0 }], ['play_tracks', { trackIds: [spec.localTrackId, spec.nextTrackId] }],
    ['seek_playback', { position: 30 }],
  ]);
});

test('wrong video fails immediately without silently choosing a replacement', async () => {
  const { result, calls } = await trial((state, time) => time >= 60_000 ? { ...state, candidate: { provider: 'youtube', id: 'youtube:zzzzzzzzzzz' } } : state);
  assert.equal(result.originalFailure, 'source_identity_mismatch'); assert.equal(result.transitionOk, false);
  assert.equal(calls.filter(([name]) => name === 'play_tracks').length, 1);
});

test('a current WebView profile is rejected before any playback mutation', async () => {
  const calls = [];
  const result = await observeTransition({ call: async (name, args) => { calls.push([name, args]); return { engine: 'webview' }; } }, spec);
  assert.equal(result.originalFailure, 'native_engine_required');
  assert.deepEqual(calls.map(([name]) => name), ['playback_state']);
});

test('control pause and premature control ending are failed attempts without another seek or skip', async () => {
  const paused = await trial((state, time) => time >= 100 ? { ...state, status: 'paused' } : state);
  assert.equal(paused.result.originalFailure, 'control_interrupted');
  const early = await trial((state, time) => time >= 100 ? { ...state,
    trackId: spec.nextTrackId, entryId: 'next-occurrence', generation: 3, position: .1, duration: 228,
    candidate: { provider: 'youtube', id: spec.nextTrackId } } : state);
  assert.equal(early.result.originalFailure, 'control_ended_early');
  assert.equal(early.calls.filter(([name]) => name === 'seek_playback').length, 1);
});

test('reordering, duplicate occurrence identities and control cancellation cannot qualify', async () => {
  for (const mutate of [
    state => ({ ...state, queueEntryIds: ['next-occurrence', 'local-occurrence'] }),
    state => ({ ...state, queueEntryIds: ['same', 'same'] }),
    state => ({ ...state, generation: 999 }),
  ]) {
    const { result } = await trial((state, time) => time >= 100 ? mutate(state) : state);
    assert.equal(result.transitionOk, false);
    assert.ok(['queue_occurrence_changed', 'control_owner_changed'].includes(result.originalFailure));
  }
});

test('original terminal failure survives a later nominally successful fixture state', async () => {
  const { result } = await trial((state, time) => time === 60_000 ? { ...state, status: 'failed',
    failure: { category: 'source_access', message: 'https://private/?token=secret' } } : state);
  assert.equal(result.originalFailure, 'original_playback_failure');
  assert.equal(result.originalPlaybackFailureCategory, 'source_access');
  assert.equal(result.samples.at(-1).atUnixMs, 60_000);
  assert.ok(!JSON.stringify(result).includes('secret'));
});

test('native recovery and next-owner replacement retain their failure instead of retrying', async () => {
  const recovered = await trial((state, time) => time >= 60_000 ? { ...state, diagnostics: { recoveryCount: 1 } } : state);
  assert.equal(recovered.result.originalFailure, 'original_recovery_observed');
  const changed = await trial((state, time) => time >= 61_000 ? { ...state, generation: 4 } : state);
  assert.equal(changed.result.originalFailure, 'next_owner_changed');
});

test('a held playhead reaches the absolute deadline and never counts as sustained PCM progress', async () => {
  const { result } = await trial((state, time) => time >= 60_000 ? { ...state, position: .1 } : state);
  assert.equal(result.transitionOk, false); assert.equal(result.originalFailure, 'transition_deadline');
  assert.equal(result.wallMs, 100_000);
});

test('buffering before first PCM is observed as transition delay, not first audio', async () => {
  const { result } = await trial((state, time) => time >= 60_000 && time < 61_000 ? { ...state, status: 'buffering', position: 0 } : state);
  assert.equal(result.transitionOk, true); assert.equal(result.observedHandoffDelayMs, 1000);
  assert.equal(result.firstNextPlaying.atUnixMs, 61_000);
});

test('safe samples discard URLs, credentials and raw diagnostics', () => {
  const sample = safeSample({ candidate: { provider: 'youtube', id: 'youtube:abcdefghijk', url: 'https://private/?token=secret' },
    diagnostics: { lastSourceFailure: { reason: 'secret' }, raw: 'secret' }, failure: { category: 'secret' } }, 1);
  assert.ok(!JSON.stringify(sample).includes('secret')); assert.equal(sample.failureCategory, null);
  assert.equal(sample.sourceFailureObserved, true);
});

test('manifest requires the exact temporary profile marker and rejects a symlinked automation endpoint', async () => {
  const profile = await mkdtemp('/tmp/burrata-launch.');
  try {
    await mkdir(join(profile, 'fixture')); await mkdir(join(profile, 'data/org.burrata.discovery'), { recursive: true });
    const automationFile = join(profile, 'data/org.burrata.discovery/automation.json');
    const manifest = join(profile, 'fixture/next-preparation.json');
    await writeFile(manifest, JSON.stringify({ ...spec, localDurationSeconds: 90, automationFile }));
    await writeFile(automationFile, '{}');
    await assert.rejects(requireIsolatedManifest(manifest));
    await writeFile(join(profile, '.burrata-isolated-profile'), 'burrata isolated memory run\n');
    assert.equal((await requireIsolatedManifest(manifest)).nextVideoId, spec.nextVideoId);
    await rm(automationFile); await symlink(manifest, automationFile);
    await assert.rejects(requireIsolatedManifest(manifest), /isolated_profile_required/);
  } finally { await rm(profile, { recursive: true, force: true }); }
});
