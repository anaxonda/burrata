import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createPrototype, decodePcm } from './core.mjs';

function fake() {
  const calls = { clients: 0, info: 0, decoded: [], formatRequests: [] };
  let time = 1000, fail = false;
  const execute = createPrototype({
    now: () => time,
    async createClient() {
      calls.clients++;
      return { session: { player: {}, user_agent: 'offline-test-agent' },
      async getBasicInfo() { calls.info++; return { basic_info: { id: 'abcdefghijk', title: 'Fixture Track', author: 'Fixture Topic', duration: 123 },
          streaming_data: { expires: new Date(time + 90_000) },
          chooseFormat: options => { calls.formatRequests.push(options); return { itag: 140, mime_type: 'audio/webm; codecs="opus"', decipher: async () => 'https://cdn.example/private-token' }; } }; },
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
  assert.equal(first.formatId, 140);
  assert.equal(first.codec, 'audio/webm; codecs="opus"');
  assert.deepEqual(first.providerMetadata, { videoId: 'abcdefghijk', title: 'Fixture Track', channel: 'Fixture Topic', durationSeconds: 123 });
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

test('strict format pin selects only exact itag and passes bounded native stream headers', async () => {
  const { execute, calls } = fake();
  const result = await execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', formatId: 140, budgetMs: 1000 });
  assert.deepEqual(calls.formatRequests[0], { itag: 140 });
  assert.equal(result.formatId, 140);
  assert.ok(result.headerNames.includes('user-agent'));
  assert.equal(calls.decoded[0].options.headers['user-agent'], 'offline-test-agent');
  assert.equal(calls.decoded[0].options.headers.referer, 'https://www.youtube.com/');
  await assert.rejects(execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', formatId: 251, budgetMs: 1000 }),
    error => error.code === 'FORMAT_MISMATCH');
  assert.equal(calls.decoded.length, 1, 'mismatched itag must not fall through to another format or decode');
  await assert.rejects(execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', formatId: 0, budgetMs: 1000 }),
    error => error.code === 'INVALID_PLAYBACK_REQUEST');
});

test('GVS token mint overlaps player preparation and is applied before extraction', async () => {
  let resolveReady, ready = false, tokenStarted = false, applied = false;
  const client = { session: { player: undefined }, playbackReady: new Promise(resolve => { resolveReady = () => { ready = true; resolve(); }; }),
    playbackPreparationSignal: new AbortController().signal,
    async getBasicInfo() {
      assert.equal(ready, true); assert.equal(applied, true);
      return { basic_info: { id: 'abcdefghijk' }, streaming_data: { expires: new Date(Date.now() + 90_000) },
        chooseFormat: () => ({ itag: 251, mime_type: 'audio/webm; codecs="opus"', decipher: async () => 'https://cdn.example/audio' }) };
    } };
  const execute = createPrototype({ createClient: async () => client,
    async ensureToken() { tokenStarted = true; return { tokenCacheHit: false, tokenHelperCpuMs: 1 }; },
    applyCachedToken() { assert.equal(ready, true); client.session.player = { po_token: 'already-minted' }; applied = true; },
    decode: async () => ({ firstPcmMs: 1, decodedFrames: 240000, decodedAudioMs: 5000 }) });
  const pending = execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', formatId: 251, budgetMs: 1000 });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(tokenStarted, true, 'token work starts while player preparation is pending');
  resolveReady();
  const result = await pending;
  assert.equal(result.formatId, 251);
});

test('cached token application waits for a fast player and the slower token mint', async () => {
  let resolveToken, tokenFinished = false, applied = false;
  const client = { session: { player: {} }, playbackReady: Promise.resolve(), async getBasicInfo() {
    return { basic_info: { id: 'abcdefghijk' }, streaming_data: { expires: new Date(Date.now() + 90_000) },
      chooseFormat: () => ({ itag: 251, mime_type: 'audio/webm; codecs="opus"', decipher: async () => 'https://cdn.example/audio' }) };
  } };
  const execute = createPrototype({ createClient: async () => client,
    ensureToken: () => new Promise(resolve => { resolveToken = () => { tokenFinished = true; resolve({ tokenHelperCpuMs: 1 }); }; }),
    applyCachedToken() { assert.equal(tokenFinished, true); applied = true; },
    decode: async () => ({ firstPcmMs: 1, decodedFrames: 240000, decodedAudioMs: 5000 }) });
  const pending = execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', budgetMs: 1000 });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(applied, false, 'fast player readiness must not race ahead of token mint');
  resolveToken();
  await pending;
  assert.equal(applied, true);
});

test('full-recording request requires diagnostic opt-in and remains capped at 600 seconds', async () => {
  let clients = 0, seen;
  const createClient = async () => { clients++; return { session: { player: {} }, async getBasicInfo() {
    return { basic_info: { id: 'abcdefghijk', title: 'Fixture Track', author: 'Fixture Topic', duration: 301 },
      streaming_data: { expires: new Date(Date.now() + 90_000) },
      chooseFormat: () => ({ itag: 251, mime_type: 'audio/webm; codecs="opus"', decipher: async () => 'https://cdn.example/audio' }) };
  } }; };
  const decode = async (_input, options) => { seen = options; return { firstPcmMs: 1, decodedFrames: 301 * 48000,
    decodedAudioMs: 301000 }; };
  const normal = createPrototype({ createClient, decode });
  await assert.rejects(normal({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk',
    seconds: 301, fullRecording: true, budgetMs: 1000 }), error => error.code === 'INVALID_PLAYBACK_REQUEST');
  assert.equal(clients, 0);

  const diagnostic = createPrototype({ createClient, decode, allowFullRecording: true });
  const result = await diagnostic({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk',
    seconds: 301, fullRecording: true, budgetMs: 1000 });
  assert.equal(seen.seconds, 302); assert.equal(seen.minimumDecodedSeconds, 300);
  assert.equal(result.decodedAudioMs, 301000);
  await assert.rejects(diagnostic({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk',
    seconds: 601, fullRecording: true, budgetMs: 1000 }), error => error.code === 'INVALID_PLAYBACK_REQUEST');
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
    const samples = 8000 * 8, wav = Buffer.alloc(44 + samples * 2);
    wav.write('RIFF'); wav.writeUInt32LE(36 + samples * 2, 4); wav.write('WAVEfmt ', 8);
    wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22);
    wav.writeUInt32LE(8000, 24); wav.writeUInt32LE(16000, 28); wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34);
    wav.write('data', 36); wav.writeUInt32LE(samples * 2, 40);
    const fixture = join(directory, 'fixture.wav'); await writeFile(fixture, wav);
    const result = await decodePcm(fixture, { position: 2, seconds: 5,
      headers: { 'user-agent': 'offline-test-agent', 'accept-language': '*', referer: 'https://www.youtube.com/' } });
    assert.ok(result.decodedFrames >= 239900 && result.decodedFrames <= 240100);
    assert.ok(result.firstPcmMs >= 0);
    // A 7.8s declaration with one-second allowance accepts this 8s PCM result.
    const endTolerance = await decodePcm(fixture, { seconds: 8.8, minimumDecodedSeconds: 6.8 });
    assert.ok(endTolerance.decodedAudioMs >= 8000);
    // The same 8s preview is rejected for a 60s full source (minimum 59s).
    await assert.rejects(decodePcm(fixture, { seconds: 61, minimumDecodedSeconds: 59 }), error => error.code === 'SHORT_AUDIO');
    // Invalid thresholds fail before FFmpeg is spawned.
    await assert.rejects(decodePcm(fixture, { seconds: 5, minimumDecodedSeconds: 6,
      ffmpeg: '/does-not-exist/ffmpeg' }), error => error.code === 'INVALID_PLAYBACK_REQUEST');
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

test('invalid strict format pin is rejected before session initialization', async () => {
  const { execute, calls } = fake();
  await assert.rejects(execute({ action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk', formatId: '251', budgetMs: 1000 }),
    error => error.code === 'INVALID_PLAYBACK_REQUEST');
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

test('decoder header allowlist rejects credential and newline injection', async () => {
  await assert.rejects(decodePcm('/not-opened', { headers: { cookie: 'secret' } }), error => error.code === 'INVALID_PLAYBACK_HEADERS');
  await assert.rejects(decodePcm('/not-opened', { headers: { 'user-agent': 'unsafe\r\nCookie: secret' } }),
    error => error.code === 'INVALID_PLAYBACK_HEADERS');
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
