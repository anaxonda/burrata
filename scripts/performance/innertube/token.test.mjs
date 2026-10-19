import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { createTokenManager } from './token.mjs';

const videoA = 'jNQXAC9IVRw', videoB = 'aqz-KE-bpKQ';

function fixture() {
  let clock = 1000, calls = [], killed = 0, spawned = 0, spawnOptions;
  const client = { session: { context: { client: { visitorData: 'private-visitor', clientName: 'WEB', clientVersion: 'test' } }, player: {} } };
  const manager = createTokenManager({ modulePath: new URL('./token_child.mjs', import.meta.url).pathname, now: () => clock,
    spawnChild(_command, _args, options) {
      spawned++;
      spawnOptions = options;
      const child = new EventEmitter(); child.stdout = new PassThrough(); child.stdin = new PassThrough();
      child.kill = () => { killed++; queueMicrotask(() => child.emit('close')); };
      child.stdin.on('data', data => {
        const request = JSON.parse(data); calls.push(request);
        queueMicrotask(() => child.stdout.write(JSON.stringify({ ok: true, poToken: `token-${request.contentBinding}`,
          contentBinding: request.contentBinding, expiresAt: clock + 90_000,
          tokenProcessCpuMs: calls.length * 3, tokenProcessMaxRssBytes: 1024 }) + '\n'));
      });
      return child;
    } });
  return { manager, client, calls: () => calls, counts: () => ({ calls: calls.length, killed, spawned }),
    spawnOptions: () => spawnOptions, setTime: value => { clock = value; } };
}

test('player token binds visitorData while GVS token binds each video ID', async () => {
  const state = fixture();
  try {
    const first = await state.manager.ensure(state.client, { videoId: videoA });
    assert.equal(state.spawnOptions().detached, false);
    assert.deepEqual(state.calls().map(row => row.contentBinding), ['private-visitor', videoA]);
    assert.equal(state.client.session.po_token, 'token-private-visitor');
    assert.equal(state.client.session.player.po_token, `token-${videoA}`);
    assert.deepEqual(first.tokenBindings, { player: 'visitorData', gvs: 'videoId' });
    const second = await state.manager.ensure(state.client, { videoId: videoA });
    assert.equal(second.tokenCacheHit, true);
    assert.equal(second.tokenHelperCpuMs, 0);

    await state.manager.ensure(state.client, { videoId: videoB });
    assert.deepEqual(state.calls().map(row => row.contentBinding), ['private-visitor', videoA, videoB]);
    assert.equal(state.client.session.po_token, 'token-private-visitor');
    assert.equal(state.client.session.player.po_token, `token-${videoB}`);
    assert.ok(!JSON.stringify(first).includes('private-'));

    state.setTime(70_000);
    await state.manager.ensure(state.client, { videoId: videoA });
    assert.equal(state.counts().calls, 5); // both expiring bindings are refreshed
    await state.manager.ensure(state.client, { videoId: videoA, refresh: true });
    assert.equal(state.counts().calls, 7);
    state.client.session.context.client.visitorData = 'another-private-visitor';
    await state.manager.ensure(state.client, { videoId: videoA });
    assert.equal(state.calls().at(-2).contentBinding, 'another-private-visitor');
    assert.equal(state.calls().at(-1).contentBinding, videoA);
  } finally { state.manager.stop(); }
});

test('tokens are not reused across client context changes and invalid IDs fail closed', async () => {
  const state = fixture();
  try {
    await state.manager.ensure(state.client, { videoId: videoA });
    state.client.session.context.client.clientVersion = 'changed-client';
    await state.manager.ensure(state.client, { videoId: videoA });
    assert.equal(state.counts().calls, 4);
    await assert.rejects(state.manager.ensure(state.client), error => error.code === 'TOKEN_BINDING_MISSING');
    await assert.rejects(state.manager.ensure(state.client, { videoId: 'bad' }), error => error.code === 'TOKEN_BINDING_MISSING');
  } finally { state.manager.stop(); }
});

test('cached GVS token can be bound after parallel player preparation without reminting', async () => {
  const state = fixture();
  try {
    state.client.session.player = undefined;
    await state.manager.ensure(state.client, { videoId: videoA });
    assert.equal(state.client.session.player, undefined);
    state.client.session.player = {};
    assert.equal(state.manager.applyCached(state.client, { videoId: videoA }), true);
    assert.equal(state.client.session.player.po_token, `token-${videoA}`);
    assert.equal(state.calls().length, 2);
    state.client.session.context.client.visitorData = 'different-context';
    assert.throws(() => state.manager.applyCached(state.client, { videoId: videoA }), error => error.code === 'TOKEN_BINDING_MISSING');
  } finally { state.manager.stop(); }
});

test('missing bundle or visitor binding fails explicitly', async () => {
  const manager = createTokenManager({ modulePath: '/does-not-exist/burrata-experiment' });
  await assert.rejects(manager.ensure({ session: { context: { client: { visitorData: 'private' } } } }, { videoId: videoA }), error => error.code === 'TOKEN_PROVIDER_UNAVAILABLE');
  await assert.rejects(manager.ensure({ session: {} }, { videoId: videoA }), error => error.code === 'TOKEN_BINDING_MISSING');
});

test('abort kills and rejects pending token generation', async () => {
  let child, killed = false;
  const manager = createTokenManager({ modulePath: new URL('./token_child.mjs', import.meta.url).pathname, spawnChild() {
    child = new EventEmitter(); child.stdout = new PassThrough(); child.stdin = new PassThrough();
    child.kill = () => { killed = true; queueMicrotask(() => child.emit('close')); };
    return child;
  } });
  const signal = new AbortController();
  const result = manager.ensure({ session: { context: { client: { visitorData: 'private', clientName: 'WEB', clientVersion: 'test' } }, player: {} } }, { videoId: videoA, signal: signal.signal });
  while (!child) await new Promise(resolve => setTimeout(resolve, 1));
  signal.abort();
  await assert.rejects(result, error => error.code === 'TOKEN_PROVIDER_FAILED');
  assert.equal(killed, true);
});
