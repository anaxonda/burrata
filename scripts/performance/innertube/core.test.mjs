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
  const play = { action: 'play', videoId: 'abcdefghijk', expectedVideoId: 'abcdefghijk' };
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
  const play = { action: 'play', videoId: 'abcdefghijk' };
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
  await assert.rejects(execute({ action: 'play', videoId: 'bad' }));
  assert.equal(calls.clients, 0);
  const result = await execute({ action: 'search', query: 'Artist Track' });
  assert.equal(result.candidates.length, 1);
  assert.equal(result.recordingVerified, undefined);
});

test('JSONL worker rejects invalid requests without touching session data', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-innertube-protocol-'));
  try {
    const result = spawnSync(process.execPath, [fileURLToPath(new URL('./worker.mjs', import.meta.url))],
      { input: '{"action":"unknown"}\n{bad}\n', encoding: 'utf8', env: { ...process.env, BURRATA_BENCH_DATA_DIR: directory } });
    assert.ifError(result.error);
    assert.equal(result.status, 0);
    assert.deepEqual(result.stdout.trim().split('\n').map(JSON.parse), [
      { ok: false, failure: 'INVALID_ACTION' }, { ok: false, failure: 'INVALID_JSON' }]);
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
