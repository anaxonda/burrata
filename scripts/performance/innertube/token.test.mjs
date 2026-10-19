import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { createTokenManager } from './token.mjs';

function fixture() {
  let clock = 1000, calls = 0, killed = 0, spawned = 0;
  const client = { session: { context: { client: { visitorData: 'private-binding' } }, player: {} } };
  const manager = createTokenManager({ modulePath: new URL('./token_child.mjs', import.meta.url).pathname, now: () => clock,
    spawnChild() {
      spawned++;
      const child = new EventEmitter(); child.stdout = new PassThrough(); child.stdin = new PassThrough();
      child.kill = () => { killed++; queueMicrotask(() => child.emit('close')); };
      child.stdin.on('data', data => {
        const request = JSON.parse(data); calls++;
        queueMicrotask(() => child.stdout.write(JSON.stringify({ ok: true, poToken: 'private-token', contentBinding: request.context.client.visitorData,
          expiresAt: clock + 90_000, tokenProcessCpuMs: calls * 3, tokenProcessMaxRssBytes: 1024 }) + '\n'));
      });
      return child;
    } });
  return { manager, client, counts: () => ({ calls, killed, spawned }), setTime: value => { clock = value; } };
}

test('bound anonymous token is attached to both player and request session, cached then refreshed on expiry', async () => {
  const state = fixture();
  try {
    const first = await state.manager.ensure(state.client);
    const second = await state.manager.ensure(state.client);
    assert.equal(first.tokenCacheHit, false); assert.equal(second.tokenCacheHit, true);
    assert.equal(first.tokenHelperCpuMs, 3); assert.equal(second.tokenHelperCpuMs, 0);
    assert.equal(state.client.session.po_token, 'private-token');
    assert.equal(state.client.session.player.po_token, 'private-token');
    assert.ok(!JSON.stringify(first).includes('private-'));
    state.setTime(70_000);
    await state.manager.ensure(state.client);
    assert.equal(state.counts().calls, 2);
    await state.manager.ensure(state.client, { refresh: true });
    assert.equal(state.counts().calls, 3);
    state.client.session.context.client.visitorData = 'another-private-binding';
    await state.manager.ensure(state.client);
    assert.equal(state.counts().calls, 4);
  } finally { state.manager.stop(); }
});

test('missing bundle or missing binding fails explicitly without pretending token support', async () => {
  const manager = createTokenManager({ modulePath: '/does-not-exist/burrata-experiment' });
  await assert.rejects(manager.ensure({ session: { context: { client: { visitorData: 'private' } } } }), error => error.code === 'TOKEN_PROVIDER_UNAVAILABLE');
  await assert.rejects(manager.ensure({ session: {} }), error => error.code === 'TOKEN_BINDING_MISSING');
});

test('abort kills and rejects pending token generation', async () => {
  let child, killed = false;
  const manager = createTokenManager({ modulePath: new URL('./token_child.mjs', import.meta.url).pathname, spawnChild() {
    child = new EventEmitter(); child.stdout = new PassThrough(); child.stdin = new PassThrough();
    child.kill = () => { killed = true; queueMicrotask(() => child.emit('close')); };
    return child;
  } });
  const signal = new AbortController();
  const result = manager.ensure({ session: { context: { client: { visitorData: 'private' } } } }, { signal: signal.signal });
  while (!child) await new Promise(resolve => setTimeout(resolve, 1));
  signal.abort();
  await assert.rejects(result, error => error.code === 'TOKEN_PROVIDER_FAILED');
  assert.equal(killed, true);
});
