import test from 'node:test';
import assert from 'node:assert/strict';
import { createAnonymousClientFactory } from './client_factory.mjs';

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
function client() {
  return { session: { context: { client: { clientName: 'WEB', clientVersion: 'test', visitorData: 'private-visitor' } } } };
}

test('parallel preparation exposes context before player and starts both public APIs', async () => {
  const context = deferred(), player = deferred();
  const events = [];
  const factory = createAnonymousClientFactory({ parallelPlayer: true, cache: 'isolated-cache',
    fetch() { throw new Error('no network in fixture'); },
    Innertube: { create(options) { events.push(['context', options.retrieve_player]); return context.promise; } },
    Player: { create(cache) { events.push(['player', cache]); return player.promise; } } });
  const pending = factory();
  assert.deepEqual(events, [['context', false], ['player', 'isolated-cache']]);
  const minimal = client();
  context.resolve(minimal);
  const result = await pending;
  assert.equal(result, minimal);
  assert.equal(result.session.player, undefined);
  let ready = false;
  result.playbackReady.then(() => { ready = true; });
  await tick();
  assert.equal(ready, false);
  const resolvedPlayer = { signature_timestamp: 123 };
  player.resolve(resolvedPlayer);
  assert.equal(await result.playbackReady, result);
  assert.equal(result.session.player, resolvedPlayer);
  assert.equal(result.playbackPreparation.mode, 'parallel-player');
});

test('default control calls ordinary Innertube.create only and preserves its player', async () => {
  let optionsSeen;
  const existing = client();
  existing.session.player = { signature_timestamp: 4 };
  const factory = createAnonymousClientFactory({ cache: 'cache', fetch() {},
    sessionOptions: { enable_session_cache: true },
    Innertube: { async create(options) { optionsSeen = options; return existing; } },
    Player: { create() { throw new Error('control must not start an extra player'); } } });
  assert.equal(await factory(), existing);
  assert.equal(optionsSeen.retrieve_player, undefined);
  assert.equal(optionsSeen.enable_session_cache, true);
  assert.equal((await existing.playbackReady).session.player.signature_timestamp, 4);
});

test('caller cancellation rejects preparation and prevents late player installation', async () => {
  const player = deferred();
  const minimal = client();
  const abort = new AbortController();
  const factory = createAnonymousClientFactory({ parallelPlayer: true, fetch() {},
    Innertube: { async create() { return minimal; } }, Player: { create() { return player.promise; } } });
  const result = await factory({ signal: abort.signal });
  const reason = Object.assign(new Error('cancelled'), { code: 'CANCELLED' });
  abort.abort(reason);
  await assert.rejects(result.playbackReady, error => error === reason);
  player.resolve({ signature_timestamp: 9 });
  await tick();
  assert.equal(result.session.player, undefined);
});

test('player rejection aborts pending context fetch and is observed before context returns', async () => {
  const player = deferred();
  const fetching = deferred();
  let observedSignal;
  const factory = createAnonymousClientFactory({ parallelPlayer: true,
    fetch(_input, init) { observedSignal = init.signal; return fetching.promise; },
    Innertube: { create(options) { return options.fetch('context'); } },
    Player: { create() { return player.promise; } } });
  const pending = factory();
  const rejected = assert.rejects(pending, /player failed/);
  player.reject(new Error('player failed'));
  await rejected;
  assert.equal(observedSignal.aborted, true);
  fetching.reject(new Error('late context failure'));
  await tick();
});

test('late context failure aborts the player fetch with no unhandled branch', async () => {
  const context = deferred(), playerFetch = deferred();
  let observedSignal;
  const factory = createAnonymousClientFactory({ parallelPlayer: true,
    fetch(_input, init) { observedSignal = init.signal; return playerFetch.promise; },
    Innertube: { create() { return context.promise; } },
    Player: { create(_cache, fetch) { return fetch('player'); } } });
  const pending = factory();
  const rejected = assert.rejects(pending, /context failed/);
  context.reject(new Error('context failed'));
  await rejected;
  assert.equal(observedSignal.aborted, true);
  playerFetch.reject(new Error('late player failure'));
  await tick();
});

test('changed session identity cannot receive a late player', async () => {
  const player = deferred();
  const minimal = client();
  const factory = createAnonymousClientFactory({ parallelPlayer: true, fetch() {},
    Innertube: { async create() { return minimal; } }, Player: { create() { return player.promise; } } });
  const result = await factory();
  result.session.context.client.visitorData = 'different-visitor';
  player.resolve({ signature_timestamp: 1 });
  await assert.rejects(result.playbackReady, error => error.code === 'CANCELLED');
  assert.equal(result.session.player, undefined);
});

test('player failure aborts the signal exposed to the sibling token operation', async () => {
  const player = deferred();
  const factory = createAnonymousClientFactory({ parallelPlayer: true, fetch() {},
    Innertube: { async create() { return client(); } }, Player: { create() { return player.promise; } } });
  const result = await factory();
  const reason = new Error('player failed while minting');
  player.reject(reason);
  await assert.rejects(result.playbackReady, error => error === reason);
  assert.equal(result.playbackPreparationSignal.aborted, true);
  assert.equal(result.playbackPreparationSignal.reason, reason);
});

test('token failure can cancel the sibling player preparation before it commits', async () => {
  const playerFetch = deferred();
  let playerSignal;
  const factory = createAnonymousClientFactory({ parallelPlayer: true,
    fetch(_input, init) { playerSignal = init.signal; return playerFetch.promise; },
    Innertube: { async create() { return client(); } },
    Player: { create(_cache, fetch) { return fetch('player'); } } });
  const result = await factory();
  const reason = new Error('token failed');
  result.abortPlaybackPreparation(reason);
  await assert.rejects(result.playbackReady, error => error === reason);
  assert.equal(playerSignal.aborted, true);
  playerFetch.resolve({ signature_timestamp: 1 });
  await tick();
  assert.equal(result.session.player, undefined);
});

test('finished preparation does not retain the first command cancellation for warm fetches', async () => {
  const abort = new AbortController();
  let delegatedFetch;
  const seen = [];
  const factory = createAnonymousClientFactory({ parallelPlayer: true,
    fetch(_input, init) { seen.push(init.signal); return Promise.resolve({ ok: true }); },
    Innertube: { async create(options) { delegatedFetch = options.fetch; return client(); } },
    Player: { async create() { return {}; } } });
  const result = await factory({ signal: abort.signal });
  await result.playbackReady;
  abort.abort(new Error('old command ended'));
  const current = new AbortController();
  await delegatedFetch('warm-request', { signal: current.signal });
  assert.equal(seen[0], current.signal);
  assert.equal(seen[0].aborted, false);
});

test('anonymous experiment rejects account-bearing options before starting work', () => {
  assert.throws(() => createAnonymousClientFactory({ sessionOptions: { cookie: 'private-cookie' } }),
    error => error.code === 'INVALID_PLAYBACK_REQUEST');
});
