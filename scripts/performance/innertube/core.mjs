import { spawn } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { commandBudget, failure } from './budget.mjs';

export async function decodePcm(input, { position = 0, seconds = 5, ffmpeg = 'ffmpeg', signal } = {}) {
  signal?.throwIfAborted();
  const started = performance.now();
  const directory = await mkdtemp(join(tmpdir(), 'burrata-decoder-accounting-'));
  const accounting = join(directory, 'usage.json');
  const args = ['-nostdin', '-hide_banner', '-loglevel', 'error', '-rw_timeout', '10000000'];
  if (position) args.push('-ss', String(position));
  args.push('-i', input, '-t', String(seconds), '-vn', '-f', 'f32le', '-acodec', 'pcm_f32le', '-ar', '48000', '-ac', '2', 'pipe:1');
  const child = spawn('python3', [fileURLToPath(new URL('./measure_child.py', import.meta.url)), accounting, ffmpeg, ...args], {
    stdio: ['ignore', 'pipe', 'pipe'], detached: process.platform !== 'win32',
  });
  let bytes = 0, firstPcmMs = null, denied = false, killTimer;
  const abort = () => {
    child.kill('SIGTERM');
    killTimer = setTimeout(() => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} }, 250);
    killTimer.unref();
  };
  signal?.addEventListener('abort', abort, { once: true });
  if (signal?.aborted) abort();
  child.stdout.on('data', chunk => { firstPcmMs ??= performance.now() - started; bytes += chunk.length; });
  child.stderr.on('data', chunk => { if (/403|410/.test(chunk.toString())) denied = true; });
  let usage;
  try {
    await new Promise((resolve, reject) => {
      let spawnError;
      child.once('error', error => { spawnError = error; });
      child.once('close', code => {
        if (signal?.aborted) reject(signal.reason);
        else if (spawnError) reject(failure('DECODER_UNAVAILABLE', 'decode'));
        else if (code !== 0) reject(failure(code === 127 ? 'DECODER_UNAVAILABLE' : denied ? 'STREAM_DENIED' : 'DECODE_FAILED', 'decode'));
        else resolve();
      });
    });
    const decodedFrames = Math.floor(bytes / 8);
    if (!decodedFrames) throw failure('NO_PCM', 'decode');
    if (decodedFrames < (seconds - 0.1) * 48000) throw failure('SHORT_AUDIO', 'decode');
    usage = JSON.parse(await readFile(accounting, 'utf8'));
    return { firstPcmMs, decodedFrames, decodedAudioMs: decodedFrames / 48, decodeWallMs: performance.now() - started,
      decoderCpuMs: usage.childCpuMs, decoderMaxRssBytes: usage.childMaxRssBytes,
      accountingWrapperCpuMs: usage.wrapperCpuMs, accountingWrapperMaxRssBytes: usage.wrapperMaxRssBytes,
      decoderAccounting: usage.accounting };
  } catch (error) {
    try { error.decoderUsage = JSON.parse(await readFile(accounting, 'utf8')); } catch {}
    throw error;
  } finally {
    signal?.removeEventListener('abort', abort); clearTimeout(killTimer);
    await rm(directory, { recursive: true, force: true });
  }
}

export async function atStage(stage, operation, stages) {
  const started = performance.now();
  try { return await operation(); }
  catch (error) { error.benchmarkStage = stage; throw error; }
  finally { if (stages) stages[stage] = (stages[stage] ?? 0) + performance.now() - started; }
}

export function createPrototype({ createClient, decode = decodePcm, ensureToken, now = Date.now, clientName = 'WEB' }) {
  let clientPromise;
  const infoCache = new Map();
  return async function execute(request, { signal, started = performance.now() } = {}) {
    const budget = commandBudget(request?.budgetMs, { signal, started });
    const stages = {};
    const stage = (name, operation) => atStage(name, () => budget.run(operation), stages);
    try {
      if (!['search', 'play', 'seek'].includes(request?.action)) throw failure('INVALID_ACTION');
      if (request.action === 'search') {
        if (typeof request.query !== 'string' || !request.query.trim() || request.query.length > 500) throw failure('INVALID_QUERY');
      } else if (!/^[A-Za-z0-9_-]{11}$/.test(request.videoId ?? '') || request.expectedVideoId !== request.videoId ||
                 !Number.isFinite(request.position ?? 0) || (request.position ?? 0) < 0 ||
                 !Number.isFinite(request.seconds ?? 5) || (request.seconds ?? 5) < 5 || (request.seconds ?? 5) > 30) {
        throw failure('INVALID_PLAYBACK_REQUEST');
      }
      const cpu = process.cpuUsage();
      if (!clientPromise) {
        const preparing = stage('session', () => createClient({ signal: budget.signal }));
        clientPromise = preparing;
        preparing.catch(() => { if (clientPromise === preparing) clientPromise = null; });
      }
      const pendingClient = clientPromise;
      const client = await stage('sessionWait', () => pendingClient);
      const preparedMs = performance.now() - started;
      if (request.action === 'search') {
        const result = await stage('search', () => client.search(request.query, { type: 'video' }));
        const candidates = (result.results ?? []).filter(row => row.type === 'Video' && /^[A-Za-z0-9_-]{11}$/.test(row.id ?? '')).slice(0, 10)
          .map(row => ({ videoId: row.id, title: String(row.title ?? ''), artist: String(row.author?.name ?? ''), durationSeconds: row.duration?.seconds ?? null }));
        return { preparedMs, searchMs: stages.search, stages, candidates };
      }
      let token = ensureToken ? await stage('token', () => ensureToken(client, { signal: budget.signal })) : { tokenMode: 'disabled' };
      let cached = infoCache.get(request.videoId);
      if (request.refresh || (cached && cached.expires <= now())) { infoCache.delete(request.videoId); cached = null; }
      let audio, format, expires, decodeStarted;
      let retryDecoderCpuMs = 0, retryWrapperCpuMs = 0, retryDecoderMaxRssBytes = 0;
      const extractionStarted = performance.now();
      let refreshedAfterDenial = false;
      for (let attempt = 0; attempt < 2; attempt++) {
        const info = cached?.info ?? await stage('extraction', () => client.getBasicInfo(request.videoId, { client: clientName }));
        const actualId = info.basic_info?.id;
        if (actualId && actualId !== request.videoId) throw failure('RECORDING_MISMATCH', 'extraction');
        // Missing expiry cannot justify caching a signed manifest.
        const providerExpiry = info.streaming_data?.expires instanceof Date ? info.streaming_data.expires.getTime() - 30_000 : null;
        expires = Number.isFinite(providerExpiry) ? providerExpiry : now();
        expires = Math.min(expires, now() + 300_000);
        if (expires > now()) {
          if (infoCache.size >= 8) infoCache.delete(infoCache.keys().next().value);
          infoCache.set(request.videoId, { info, expires });
        }
        try {
          if (Number.isFinite(providerExpiry) && providerExpiry <= now()) throw failure('MANIFEST_EXPIRED', 'extraction');
          format = await stage('format', () => info.chooseFormat({ type: 'audio', quality: 'best', format: 'any' }));
          const stream = await stage('decipher', () => format.decipher(client.session.player));
          const parsed = new URL(stream);
          const urlExpiry = Number(parsed.searchParams.get('expire')) * 1000;
          if (urlExpiry > 0) expires = Math.min(expires, urlExpiry - 30_000);
          if (urlExpiry > 0 && urlExpiry <= now() + 30_000) throw failure('MANIFEST_EXPIRED', 'decipher');
          if (expires <= now()) infoCache.delete(request.videoId);
          else if (infoCache.has(request.videoId)) infoCache.get(request.videoId).expires = expires;
          decodeStarted = performance.now();
          audio = await stage('decode', () => decode(stream, { position: request.position ?? 0, seconds: request.seconds ?? 5, signal: budget.signal }));
          break;
        } catch (error) {
          infoCache.delete(request.videoId);
          retryDecoderCpuMs += error.decoderUsage?.childCpuMs ?? 0;
          retryWrapperCpuMs += error.decoderUsage?.wrapperCpuMs ?? 0;
          retryDecoderMaxRssBytes = Math.max(retryDecoderMaxRssBytes, error.decoderUsage?.childMaxRssBytes ?? 0);
          if (!['STREAM_DENIED', 'MANIFEST_EXPIRED'].includes(error.code) || attempt === 1) throw error;
          refreshedAfterDenial = true; cached = null;
          if (ensureToken) {
            const refreshed = await stage('token', () => ensureToken(client, { signal: budget.signal, refresh: true }));
            token = { ...refreshed, tokenHelperCpuMs: (token.tokenHelperCpuMs ?? 0) + (refreshed.tokenHelperCpuMs ?? 0) };
          }
        }
      }
      const used = process.cpuUsage(cpu);
      const helperCpuMs = (used.user + used.system) / 1000;
      return { clientName, preparedMs, extractionMs: performance.now() - extractionStarted - stages.decode,
        infoCacheHit: Boolean(cached), manifestExpiresInMs: Math.max(0, expires - now()), refreshedAfterDenial,
        ...audio, firstPcmFromCommandMs: Number.isFinite(audio.firstPcmMs) ? decodeStarted - started + audio.firstPcmMs : null,
        decoderCpuMs: (audio.decoderCpuMs ?? 0) + retryDecoderCpuMs,
        decoderMaxRssBytes: Math.max(audio.decoderMaxRssBytes ?? 0, retryDecoderMaxRssBytes),
        accountingWrapperCpuMs: (audio.accountingWrapperCpuMs ?? 0) + retryWrapperCpuMs,
        ...token, stages, helperCpuMs, helperRssBytes: process.memoryUsage().rss,
        helperMaxRssBytes: process.resourceUsage().maxRSS * 1024,
        accountingComplete: Number.isFinite(audio.decoderCpuMs) && Number.isFinite(audio.accountingWrapperCpuMs) && (!ensureToken || Number.isFinite(token.tokenHelperCpuMs)),
        totalCpuMs: helperCpuMs + (token.tokenHelperCpuMs ?? 0) + (audio.decoderCpuMs ?? 0) + (audio.accountingWrapperCpuMs ?? 0) + retryDecoderCpuMs + retryWrapperCpuMs,
        wallMs: performance.now() - started, codec: /^audio\/[\w.+-]+(?:; codecs="[\w., -]+")?$/.test(format.mime_type ?? '') ? format.mime_type : null, recordingVerified: true,
        identityScope: 'explicit-provider-video-id; not acoustic verification' };
    } catch (error) {
      if (budget.signal.aborted) { clientPromise = null; infoCache.clear(); }
      error.stages = stages;
      throw error;
    } finally { budget.close(); }
  };
}
