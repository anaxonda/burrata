import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { assessEntryPointPlayback, playVisibleSearchOccurrence, validateEntryPointSpec, validateUiAction } from './playback_entrypoint_audit.mjs';

const spec = {
  entrypoint: 'library_row', route: 'library', trackId: 'spotify:track123',
  expectedProvider: 'youtube', expectedRecordingId: 'youtube:recording123', expectedDurationSeconds: 245.48,
};

test('audit search enums conform to the native renderer API contract', async () => {
  const native = await readFile(new URL('../../src-tauri/src/automation.rs', import.meta.url), 'utf8');
  const helper = await readFile(new URL('./playback_entrypoint_audit.mjs', import.meta.url), 'utf8');
  for (const [nativeName, helperName] of [['SEARCH_UI_KINDS', 'SEARCH_KINDS'], ['SEARCH_UI_SOURCE_IDS', 'SEARCH_SOURCES']]) {
    const nativeArray = native.match(new RegExp(`const ${nativeName}[^=]*=\\s*&\\[([\\s\\S]*?)\\];`));
    const helperArray = helper.match(new RegExp(`const ${helperName} = new Set\\((\\[[^\\]]*\\])\\)`));
    assert.ok(nativeArray && helperArray);
    const expected = [...nativeArray[1].matchAll(/"([^"]+)"/g)].map(match => match[1]);
    const actual = JSON.parse(helperArray[1].replaceAll("'", '"'));
    assert.deepEqual(actual.sort(), expected.sort());
  }
});

test('audit accepts a bounded saved-Library row entrypoint spec', () => {
  assert.equal(validateEntryPointSpec(spec), spec);
});

test('audit supports the renderer search result route and limits query length', () => {
  const search = { ...spec, entrypoint: 'search_result', route: undefined, query: 'Artist - Track' };
  assert.equal(validateEntryPointSpec(search), search);
  assert.throws(() => validateEntryPointSpec({ ...search, query: 'x'.repeat(501) }), /invalid_search_query/);
});

test('search entrypoint plays the one exact visible renderer occurrence by expiring token', async () => {
  const calls = [];
  const call = async (name, args) => {
    calls.push([name, args]);
    if (name === 'search_ui') return { result: { status: 'complete' } };
    if (name === 'inspect_search_ui') return { result: { status: 'ready', mode: 'online', revision: '123e4567-e89b-12d3-a456-426614174000', rows: [
      { occurrenceToken: '123e4567-e89b-12d3-a456-426614174001', sourceId: 'youtube', title: 'Track', artist: 'Artist', durationSeconds: 245.5 },
    ] } };
    return { result: { ok: true } };
  };
  const result = await playVisibleSearchOccurrence(call, { entrypoint: 'search_result', query: 'Artist Track',
    searchSourceId: 'youtube', kind: 'tracks', title: 'Track', artist: 'Artist', expectedDurationSeconds: 245.48 });
  assert.equal(result.action, 'renderer_occurrence_token');
  assert.deepEqual(calls.map(([name]) => name), ['search_ui', 'inspect_search_ui', 'play_search_result']);
  assert.equal(calls[2][1].occurrenceToken, '123e4567-e89b-12d3-a456-426614174001');
});

test('search entrypoint refuses absent or ambiguous visible occurrences', async () => {
  const call = async (name) => name === 'search_ui' ? { result: { status: 'complete' } } : { result: {
    status: 'ready', mode: 'online', revision: '123e4567-e89b-12d3-a456-426614174000', rows: [],
  } };
  await assert.rejects(playVisibleSearchOccurrence(call, { entrypoint: 'search_result', query: 'x',
    searchSourceId: 'youtube', kind: 'tracks', title: 'Track', artist: 'Artist', expectedDurationSeconds: 245 }), /search_occurrence_missing/);
});

test('Spotify-imported saved rows can be selected through the visible Library search results', async () => {
  const calls = [];
  const call = async (name) => {
    calls.push(name);
    if (name === 'search_ui') return { result: { status: 'complete' } };
    if (name === 'inspect_search_ui') return { result: { status: 'ready', mode: 'library',
      revision: '123e4567-e89b-12d3-a456-426614174010', rows: [
        { occurrenceToken: '123e4567-e89b-12d3-a456-426614174011', sourceId: 'library',
          title: 'Revelator', artist: 'Gillian Welch', durationSeconds: 380 },
      ] } };
    return { result: { ok: true } };
  };
  const result = await playVisibleSearchOccurrence(call, { entrypoint: 'search_result', query: 'Gillian Welch Revelator',
    searchSourceId: 'library', kind: 'tracks', title: 'Revelator', artist: 'Gillian Welch', expectedDurationSeconds: 380 });
  assert.equal(result.searchMode, 'library');
  assert.deepEqual(calls, ['search_ui', 'inspect_search_ui', 'play_search_result']);
});

test('audit rejects unknown entrypoints, routes, identities, and non-Library row route', () => {
  assert.throws(() => validateEntryPointSpec({ ...spec, entrypoint: 'play_tracks' }), /invalid_entrypoint/);
  assert.throws(() => validateEntryPointSpec({ ...spec, route: 'remote-import' }), /invalid_route/);
  assert.throws(() => validateEntryPointSpec({ ...spec, expectedRecordingId: '../normal-db' }), /invalid_expected_identity/);
  assert.throws(() => validateEntryPointSpec({ ...spec, route: 'history' }), /library_row_requires_library_route/);
});

test('private UI actions are bounded left clicks only', () => {
  assert.deepEqual(validateUiAction({ id: 1, kind: 'click', x: 250, y: 450 }),
    { id: 1, kind: 'click', x: 250, y: 450, button: 1 });
  for (const action of [
    { id: 1, kind: 'click', x: -1, y: 1 },
    { id: 1, kind: 'click', x: 1, y: 900 },
    { id: 1, kind: 'click', x: 1, y: 1, button: 3 },
    { id: 1, kind: 'key', key: 'Enter' },
  ]) assert.throws(() => validateUiAction(action), /invalid_ui_action/);
});

const before = { engine: 'native', trackId: 'spotify:track123', generation: 4 };
const after = { engine: 'native', trackId: 'spotify:track123', generation: 5, status: 'playing', position: 3.1,
  duration: 245.8, candidate: { provider: 'youtube', id: 'youtube:recording123' },
  diagnostics: { firstAudioMs: 1820 } };

test('real entrypoint playback requires expected source identity, generation, startup, and duration', () => {
  const result = assessEntryPointPlayback(before, after, spec);
  assert.equal(result.status, 'ready');
  assert.equal(result.entrypoint, 'library_row');
  assert.equal(result.acousticContentVerified, false);
});

test('wrong source identity and silent startup remain failures', () => {
  assert.deepEqual(assessEntryPointPlayback(before, { ...after, candidate: { provider: 'soundcloud', id: 'soundcloud:x' } }, spec),
    { status: 'failed', reason: 'wrong_provider' });
  assert.deepEqual(assessEntryPointPlayback(before, { ...after, diagnostics: {} }, spec),
    { status: 'failed', reason: 'startup_not_observed' });
});
