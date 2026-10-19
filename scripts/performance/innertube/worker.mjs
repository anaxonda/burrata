// Isolated JSONL experiment. Never imported by Burrata's production resolver.
import { createInterface } from 'node:readline';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { Script } from 'node:vm';
import { performance } from 'node:perf_hooks';
import { createPrototype, decodePcm } from './core.mjs';
import { createTokenManager } from './token.mjs';
import { createAnonymousClientFactory } from './client_factory.mjs';
import { failure } from './budget.mjs';

const directory = process.env.BURRATA_BENCH_DATA_DIR;
if (!directory) throw new Error('BURRATA_BENCH_DATA_DIR must identify an isolated scratch directory');
let currentSignal, active, idleTimer, closed = false, tail = Promise.resolve();
let queued = 0;
const idleMs = Number(process.env.BURRATA_BENCH_IDLE_MS ?? 30_000);
if (!Number.isInteger(idleMs) || idleMs < 10 || idleMs > 300_000) throw new Error('Invalid experimental idle timeout');
const clientName = process.env.BURRATA_BENCH_CLIENT ?? 'WEB';
if (!['WEB', 'MWEB', 'ANDROID', 'IOS', 'TV'].includes(clientName)) throw new Error('Unsupported experimental client');
const tokenManager = createTokenManager();
const tokenMode = process.env.BURRATA_BENCH_TOKEN_MODE ?? 'bgutil';
if (!['bgutil', 'none'].includes(tokenMode)) throw new Error('Unsupported experimental token mode');
const parallelPlayer = process.env.BURRATA_BENCH_PARALLEL_PLAYER === '1';
let createClientFactory;
const execute = createPrototype({
  clientName,
  ensureToken: tokenMode === 'bgutil' ? tokenManager.ensure : undefined,
  applyCachedToken: tokenMode === 'bgutil' ? tokenManager.applyCached : undefined,
  async createClient(options) {
    const { Innertube, Player, UniversalCache, Platform, Log } = await import('youtubei.js');
    Log.setLevel(Log.Level.NONE);
    Platform.shim.eval = data => new Script(`(function(){${data.output}\n})()`).runInNewContext(Object.create(null), { timeout: 1000 });
    const cache = resolve(directory, 'youtube-session');
    await mkdir(cache, { recursive: true, mode: 0o700 });
    const requestFetch = (input, init = {}) => fetch(input, { ...init,
      signal: init.signal ? AbortSignal.any([init.signal, currentSignal]) : currentSignal });
    createClientFactory ??= createAnonymousClientFactory({ Innertube, Player,
      cache: new UniversalCache(true, cache), fetch: requestFetch,
      sessionOptions: { enable_session_cache: true }, parallelPlayer });
    return createClientFactory(options);
  },
  decode: (input, options) => decodePcm(input, { ...options, ffmpeg: process.env.BURRATA_BENCH_FFMPEG ?? 'ffmpeg' }),
});
const errors = new Set(['INVALID_ACTION', 'INVALID_QUERY', 'INVALID_PLAYBACK_REQUEST', 'INVALID_BUDGET', 'INVALID_PLAYBACK_HEADERS', 'FORMAT_UNAVAILABLE', 'FORMAT_MISMATCH', 'RECORDING_MISMATCH', 'STREAM_DENIED', 'MANIFEST_EXPIRED', 'DECODE_FAILED', 'DECODER_UNAVAILABLE', 'NO_PCM', 'SHORT_AUDIO', 'TIMEOUT', 'CANCELLED', 'TOKEN_PROVIDER_FAILED', 'TOKEN_PROVIDER_UNAVAILABLE', 'TOKEN_BINDING_MISSING', 'INVALID_JSON', 'QUEUE_FULL']);
const stages = new Set(['session', 'sessionWait', 'search', 'token', 'playbackPreparation', 'extraction', 'format', 'decipher', 'decode']);
function redact(error) {
  return { failure: errors.has(error.code) ? error.code : 'PROVIDER_FAILED',
    stage: stages.has(error.benchmarkStage) ? error.benchmarkStage : undefined,
    stages: error.stages, accountingComplete: false };
}
function emit(value) { process.stdout.write(JSON.stringify(value) + '\n'); }
function stop() { clearTimeout(idleTimer); active?.controller.abort(failure('CANCELLED')); tokenManager.stop(); }
function armIdle() {
  clearTimeout(idleTimer);
  if (!queued && !closed) idleTimer = setTimeout(() => { stop(); process.exit(0); }, idleMs);
}
process.once('SIGTERM', () => { stop(); process.exit(0); });
process.once('SIGINT', () => { stop(); process.exit(0); });
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on('line', line => {
  clearTimeout(idleTimer);
  const started = performance.now();
  let request;
  try { if (line.length > 16_384) throw new Error(); request = JSON.parse(line); }
  catch { emit({ ok: false, ...redact(failure('INVALID_JSON')) }); armIdle(); return; }
  if (!request || typeof request !== 'object' || Array.isArray(request)) { emit({ ok: false, ...redact(failure('INVALID_ACTION')) }); armIdle(); return; }
  const id = typeof request.id === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(request.id) ? request.id : undefined;
  if (request.action === 'cancel') {
    if (!Number.isInteger(request.budgetMs) || request.budgetMs < 1 || request.budgetMs > 120_000) { emit({ id, ok: false, ...redact(failure('INVALID_BUDGET')) }); armIdle(); return; }
    const cancelled = Boolean(active?.id && request.targetId === active.id);
    if (cancelled) active.controller.abort(failure('CANCELLED'));
    emit({ id, ok: true, cancelled }); armIdle(); return;
  }
  if (queued >= 8) { emit({ id, ok: false, ...redact(failure('QUEUE_FULL')) }); armIdle(); return; }
  queued++;
  tail = tail.then(async () => {
    const controller = new AbortController();
    currentSignal = controller.signal; active = { id, controller };
    const remaining = Number.isInteger(request.budgetMs) ? Math.max(0, request.budgetMs - (performance.now() - started)) : 0;
    const timer = setTimeout(() => controller.abort(failure('TIMEOUT')), remaining);
    try { emit({ id, ok: true, ...await execute(request, { signal: controller.signal, started }), tokenMode }); }
    catch (error) {
      if (controller.signal.aborted) { error = Object.assign(controller.signal.reason, { stages: error.stages, benchmarkStage: error.benchmarkStage }); tokenManager.stop(); }
      emit({ id, ok: false, ...redact(error) });
    } finally { clearTimeout(timer); active = null; queued--; armIdle(); }
  }).catch(() => { stop(); process.exitCode = 1; });
});
input.once('close', () => { closed = true; tail.finally(stop); });
armIdle();
