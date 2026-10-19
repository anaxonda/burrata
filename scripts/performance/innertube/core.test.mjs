import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createPrototype, decodePcm } from './core.mjs';

function fake() {
  const calls = { clients: 0, info: 0, decoded: [] };
  let time = 1000, fail = false;
  const execute = createPrototype({
    now: () => time,
    async createClient() {
      calls.clients++;
      return { session: { player: {} },
        async getBasicInfo() { calls.info++; return { streaming_data: { expires: new Date(time + 90_000) },
          chooseFormat: () => ({ mime_type: 'audio/webm; codecs="opus"', decipher: async () => 'https://cdn.example/private-token' }) }; },
        async search() { return { results: [{ type: 'Video', id: 'abcdefghijk', title: 'Track' }, { type: 'Channel', id: 'ignore' }] }; } };
    },
    async decode(input, options) { calls.decoded.push({ input, options }); if (fail) throw new Error('private-token');
      return { firstPcmMs: 1, decodedFrames: 240000, decodedAudioMs: 5000 }; },
  });
  return { execute, calls, setTime: value => { time = value; }, fail: value => { fail = value; } };
}

test('persistent client and unexpired extraction are reused across seeks without exposing URLs', async () => {
  const { execute, calls } = fake();
  const play = { action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', budgetMs: 1000 };
  const first = await execute(play);
  const seek = await execute({ ...play, action: 'seek', position: 60 });
  assert.equal(calls.clients, 1); assert.equal(calls.info, 1);
  assert.equal(first.infoCacheHit, false); assert.equal(seek.infoCacheHit, true);
  assert.equal(seek.recordingVerified, true);
  assert.equal(calls.decoded[1].options.position, 60);
  assert.ok(!JSON.stringify(seek).includes('private-token'));
});

test('expired and denied streams are resolved again', async () => {
  const state = fake();
  const play = { action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', budgetMs: 1000 };
  await state.execute(play);
  state.setTime(70_000);
  await state.execute(play);
  assert.equal(state.calls.info, 2);
  state.fail(true);
  await assert.rejects(state.execute(play));
  state.fail(false);
  await state.execute(play);
  assert.equal(state.calls.info, 3);
});

test('search candidates remain unverified and invalid requests do not initialize a client', async () => {
  const { execute, calls } = fake();
  await assert.rejects(execute({ action: 'play', videoId: 'bad', budgetMs: 1000 }));
  assert.equal(calls.clients, 0);
  const result = await execute({ action: 'search', query: 'Artist Track', budgetMs: 1000 });
  assert.equal(result.candidates.length, 1);
  assert.equal(result.recordingVerified, undefined);
});

test('JSONL worker rejects invalid requests without touching session data', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-innertube-protocol-'));
  try {
    const result = spawnSync(process.execPath, [fileURLToPath(new URL('./worker.mjs', import.meta.url))],
      { input: '{"action":"unknown","budgetMs":1000}\n{bad}\n', encoding: 'utf8', env: { ...process.env, BURRATA_BENCH_DATA_DIR: directory } });
    assert.ifError(result.error);
    assert.equal(result.status, 0);
    assert.deepEqual(result.stdout.trim().split('\n').map(JSON.parse).sort((a,b) => a.failure.localeCompare(b.failure)), [
      { ok: false, failure: 'INVALID_ACTION', stages: {}, accountingComplete: false }, { ok: false, failure: 'INVALID_JSON', accountingComplete: false }]);
    assert.deepEqual(await readdir(directory), []);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('FFmpeg probe decodes five seconds of real PCM rather than accepting extraction metadata', { timeout: 15000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-innertube-pcm-'));
  try {
    const samples = 8000 * 5, wav = Buffer.alloc(44 + samples * 2);
    wav.write('RIFF'); wav.writeUInt32LE(36 + samples * 2, 4); wav.write('WAVEfmt ', 8);
    wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22);
    wav.writeUInt32LE(8000, 24); wav.writeUInt32LE(16000, 28); wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34);
    wav.write('data', 36); wav.writeUInt32LE(samples * 2, 40);
    const fixture = join(directory, 'fixture.wav'); await writeFile(fixture, wav);
    const result = await decodePcm(fixture);
    assert.ok(result.decodedFrames >= 239900 && result.decodedFrames <= 240100);
    assert.ok(result.firstPcmMs >= 0);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('absolute budget rejects a never-settling provider and does not reset for stages', async () => {
  const execute = createPrototype({ createClient: () => new Promise(() => {}) });
  const started = Date.now();
  await assert.rejects(execute({ action: 'search', query: 'test', budgetMs: 25 }), error => error.code === 'TIMEOUT' && error.benchmarkStage === 'sessionWait');
  assert.ok(Date.now() - started < 500);
  await assert.rejects(execute({ action: 'search', query: 'test' }), error => error.code === 'INVALID_BUDGET');
});

test('cancellation interrupts a pending provider and pre-cancel avoids client creation', async () => {
  let calls = 0;
  const execute = createPrototype({ createClient: () => { calls++; return new Promise(() => {}); } });
  const abort = new AbortController();
  const result = execute({ action: 'search', query: 'test', budgetMs: 1000 }, { signal: abort.signal });
  abort.abort(Object.assign(new Error(), { code: 'CANCELLED' }));
  await assert.rejects(result, error => error.code === 'CANCELLED');
  const count = calls;
  await assert.rejects(execute({ action: 'search', query: 'test', budgetMs: 1000 }, { signal: abort.signal }));
  assert.equal(calls, count);
});

test('explicit recording mismatch is rejected before session initialization', async () => {
  const { execute, calls } = fake();
  await assert.rejects(execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'zzzzzzzzzzz', budgetMs: 1000 }), error => error.code === 'INVALID_PLAYBACK_REQUEST');
  assert.equal(calls.clients, 0);
});

test('one denied stream refreshes manifest and token inside the same budget', async () => {
  let infos = 0, decodes = 0, refreshes = 0;
  const execute = createPrototype({
    createClient: async () => ({ session: { player: {} }, getBasicInfo: async () => {
      infos++;
      return { basic_info: { id: 'abcdefghijk' }, streaming_data: { expires: new Date(Date.now() + 90_000) },
        chooseFormat: () => ({ mime_type: 'audio/webm', decipher: async () => 'https://example.com/token-private' }) };
    } }),
    ensureToken: async (_client, options) => { if (options.refresh) refreshes++; return { tokenHelperCpuMs: 2 }; },
    decode: async () => {
      if (++decodes === 1) throw Object.assign(new Error('SECRET'), { code: 'STREAM_DENIED', decoderUsage: { childCpuMs: 3, wrapperCpuMs: 4 } });
      return { firstPcmMs: 1, decoderCpuMs: 5, accountingWrapperCpuMs: 6 };
    },
  });
  const result = await execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', budgetMs: 1000 });
  assert.equal(infos, 2); assert.equal(refreshes, 1); assert.equal(result.refreshedAfterDenial, true);
  assert.equal(result.decoderCpuMs, 8); assert.equal(result.accountingWrapperCpuMs, 10); assert.equal(result.tokenHelperCpuMs, 4);
  assert.ok(!JSON.stringify(result).includes('SECRET'));
});

test('expired signed URL refreshes once before any decode and identity response is checked', async () => {
  let infos = 0, decodes = 0;
  const execute = createPrototype({ createClient: async () => ({ session: { player: {} }, getBasicInfo: async () => {
    infos++;
    return { basic_info: { id: 'abcdefghijk' }, streaming_data: { expires: new Date(Date.now() + 90_000) },
      chooseFormat: () => ({ decipher: async () => `https://example.com/audio?expire=${infos === 1 ? 1 : Math.floor(Date.now() / 1000) + 90}` }) };
  } }), decode: async () => { decodes++; return {}; } });
  const result = await execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', budgetMs: 1000 });
  assert.equal(infos, 2); assert.equal(decodes, 1); assert.equal(result.refreshedAfterDenial, true);
});

test('worker exits on idle without creating session files', async () => {
  const { spawn } = await import('node:child_process');
  const directory = await mkdtemp(join(tmpdir(), 'burrata-idle-'));
  try {
    const child = spawn(process.execPath, [fileURLToPath(new URL('./worker.mjs', import.meta.url))], {
      env: { ...process.env, BURRATA_BENCH_DATA_DIR: directory, BURRATA_BENCH_IDLE_MS: '20' }, stdio: ['pipe', 'pipe', 'pipe'],
    });
    const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
    assert.equal(code, 0);
    assert.deepEqual(await readdir(directory), []);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('missing FFmpeg is a typed decoder availability failure with wrapper accounting', async () => {
  await assert.rejects(decodePcm('/not-opened', { ffmpeg: '/does-not-exist/burrata-ffmpeg' }), error =>
    error.code === 'DECODER_UNAVAILABLE' && error.decoderUsage.childUnavailable === true);
});

test('cancellation terminates a real decoder subprocess within the kill grace', { timeout: 3000 }, async () => {
  const { chmod } = await import('node:fs/promises');
  const directory = await mkdtemp(join(tmpdir(), 'burrata-decoder-cancel-'));
  try {
    const executable = join(directory, 'controlled-decoder');
    await writeFile(executable, '#!/bin/sh\nsleep 20\n'); await chmod(executable, 0o700);
    const signal = new AbortController();
    const result = decodePcm('/not-opened', { ffmpeg: executable, signal: signal.signal });
    setTimeout(() => signal.abort(Object.assign(new Error(), { code: 'CANCELLED' })), 100);
    const started = Date.now();
    await assert.rejects(result, error => error.code === 'CANCELLED');
    assert.ok(Date.now() - started < 1500);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('deadline catches synchronous work that delays the timer callback', async () => {
  const { commandBudget } = await import('./budget.mjs');
  const budget = commandBudget(5);
  try {
    await assert.rejects(budget.run(() => { const end = Date.now() + 25; while (Date.now() < end) {} return 'late'; }), error => error.code === 'TIMEOUT');
  } finally { budget.close(); }
});
