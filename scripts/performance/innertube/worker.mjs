// Isolated JSONL experiment. Never imported by Burrata's production resolver.
import { createInterface } from 'node:readline';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { Script } from 'node:vm';
import { createPrototype, decodePcm } from './core.mjs';

const directory = process.env.BURRATA_BENCH_DATA_DIR;
if (!directory) throw new Error('BURRATA_BENCH_DATA_DIR must identify an isolated scratch directory');
let currentSignal;
const clientName = process.env.BURRATA_BENCH_CLIENT ?? 'WEB';
if (!['WEB', 'MWEB', 'ANDROID', 'IOS', 'TV'].includes(clientName)) throw new Error('Unsupported experimental client');
const execute = createPrototype({
  clientName,
  async createClient() {
    const { Innertube, UniversalCache, Platform, Log } = await import('youtubei.js');
    Log.setLevel(Log.Level.NONE);
    Platform.shim.eval = data => new Script(`(function(){${data.output}\n})()`).runInNewContext(Object.create(null), { timeout: 1000 });
    const cache = resolve(directory, 'youtube-session');
    await mkdir(cache, { recursive: true });
    return Innertube.create({ cache: new UniversalCache(true, cache), enable_session_cache: true,
      po_token: process.env.BURRATA_BENCH_PO_TOKEN,
      visitor_data: process.env.BURRATA_BENCH_VISITOR_DATA,
      fetch: (input, init = {}) => fetch(input, { ...init, signal: init.signal ? AbortSignal.any([init.signal, currentSignal]) : currentSignal }) });
  },
  decode: (input, options) => decodePcm(input, { ...options, ffmpeg: process.env.BURRATA_BENCH_FFMPEG ?? 'ffmpeg' }),
});
const allowedErrors = new Set(['INVALID_ACTION', 'INVALID_QUERY', 'INVALID_PLAYBACK_REQUEST', 'STREAM_DENIED', 'DECODE_FAILED', 'NO_PCM', 'SHORT_AUDIO']);
for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), 45_000);
  currentSignal = abort.signal;
  try {
    const request = JSON.parse(line);
    const result = await execute(request, { signal: abort.signal });
    process.stdout.write(JSON.stringify({ ok: true, ...result }) + '\n');
  } catch (error) {
    const failure = abort.signal.aborted ? 'TIMEOUT' : allowedErrors.has(error.code) ? error.code : error instanceof SyntaxError ? 'INVALID_JSON' : 'PROVIDER_FAILED';
    const stage = ['session', 'search', 'extraction', 'format', 'decipher', 'decode'].includes(error.benchmarkStage) ? error.benchmarkStage : undefined;
    const errorType = stage && ['ReferenceError', 'TypeError', 'SyntaxError', 'PlayerError', 'InnertubeError', 'Error', 'AbortError'].includes(error.name) ? error.name : undefined;
    const identifier = /^([A-Za-z_$][\w$]*) is not defined$/.exec(error.message ?? '')?.[1];
    const missingGlobal = ['URL', 'URLSearchParams', 'TextEncoder', 'TextDecoder', 'atob', 'btoa', 'performance', 'crypto', 'navigator', 'window', 'self', 'document'].includes(identifier) ? identifier : undefined;
    const reasons = new Map([['No n/sig decipher function extracted', 'NO_DECIPHER_FUNCTION'], ['No valid URL to decipher', 'NO_STREAM_URL'], ['Got invalid result from player script evaluation.', 'INVALID_EVALUATOR_RESULT'], ['Got invalid signature from player script evaluation.', 'INVALID_SIGNATURE'], ['Failed to decipher nsig', 'INVALID_NSIG']]);
    const reason = reasons.get(error.message) ?? (error.code === 'ERR_SCRIPT_EXECUTION_TIMEOUT' ? 'EVALUATOR_TIMEOUT' : undefined);
    process.stdout.write(JSON.stringify({ ok: false, failure, stage, errorType, missingGlobal, reason }) + '\n');
  } finally { clearTimeout(timer); }
}
