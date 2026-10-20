import { spawn } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { commandBudget, failure } from './budget.mjs';

export function playbackHeaders(client) {
  const headers = {
    accept: '*/*', 'accept-language': '*',
    origin: 'https://www.youtube.com', referer: 'https://www.youtube.com/', dnt: '?1',
  };
  const userAgent = client.session?.user_agent ?? client.session?.context?.client?.userAgent;
  if (typeof userAgent === 'string' && userAgent.length > 0) headers['user-agent'] = userAgent;
  for (const [key, value] of Object.entries(headers)) {
    if (/[\r\n]/.test(key) || /[\r\n]/.test(value) || value.length > 1024) throw failure('INVALID_PLAYBACK_HEADERS', 'format');
  }
  return headers;
}

export async function decodePcm(input, { position = 0, seconds = 5, minimumDecodedSeconds, ffmpeg = 'ffmpeg', headers = {}, signal } = {}) {
  const minimumSeconds = minimumDecodedSeconds ?? seconds - 0.1;
  if (!Number.isFinite(seconds) || seconds <= 0 || seconds > 601 || !Number.isFinite(minimumSeconds) ||
      minimumSeconds < 0 || minimumSeconds > seconds) throw failure('INVALID_PLAYBACK_REQUEST', 'decode');
  signal?.throwIfAborted();
  const started = performance.now();
  const allowedHeaders = new Set(['accept', 'accept-language', 'dnt', 'origin', 'referer', 'user-agent']);
  const headerLines = Object.entries(headers).map(([name, value]) => {
    if (!allowedHeaders.has(name.toLowerCase()) || typeof value !== 'string' || /[\r\n]/.test(name + value) || value.length > 1024)
      throw failure('INVALID_PLAYBACK_HEADERS', 'decode');
    return `${name}: ${value}\r\n`;
  }).join('');
  const directory = await mkdtemp(join(tmpdir(), 'burrata-decoder-accounting-'));
  const accounting = join(directory, 'usage.json');
  const args = ['-nostdin', '-hide_banner', '-loglevel', 'error', '-rw_timeout', '10000000'];
  if (position) args.push('-ss', String(position));
  // FFmpeg's HTTP protocol option is invalid for local files and other
  // non-HTTP inputs; the native playback path still forwards it for URLs.
  if (headerLines && /^https?:\/\//i.test(input)) args.push('-headers', headerLines);
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
    if (decodedFrames < minimumSeconds * 48000) throw failure('SHORT_AUDIO', 'decode');
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

export function createPrototype({ createClient, decode = decodePcm, ensureToken, applyCachedToken, allowFullRecording = false, now = Date.now, clientName = 'WEB' }) {
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
                 !Number.isFinite(request.seconds ?? 5) || (request.seconds ?? 5) < 5 ||
                 (request.fullRecording !== undefined && typeof request.fullRecording !== 'boolean') ||
                 (request.fullRecording === true && !allowFullRecording) ||
                 (request.fullRecording === true && (request.seconds ?? 5) > 600) ||
                 (request.fullRecording !== true && (request.seconds ?? 5) > 30) ||
                 (request.formatId !== undefined && (!Number.isInteger(request.formatId) || request.formatId < 1 || request.formatId > 999_999))) {
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
      let token = { tokenMode: 'disabled' };
      if (ensureToken) {
        const tokenSignal = client.playbackPreparationSignal
          ? AbortSignal.any([budget.signal, client.playbackPreparationSignal]) : budget.signal;
        const tokenTask = stage('token', () => ensureToken(client, { videoId: request.videoId, signal: tokenSignal }));
        const playbackTask = stage('playbackPreparation', async () => {
          if (client.playbackReady) await client.playbackReady;
          return true;
        });
        try {
          [token] = await Promise.all([tokenTask, playbackTask]);
          if (applyCachedToken) applyCachedToken(client, { videoId: request.videoId });
        }
        catch (error) {
          client.abortPlaybackPreparation?.(error);
          if (clientPromise === pendingClient) clientPromise = null;
          infoCache.clear();
          // Both branches are already observed by Promise.all; settle them before
          // dropping this context so no player/token work leaks into the next call.
          await Promise.allSettled([tokenTask, playbackTask]);
          throw error;
        }
      } else if (client.playbackReady) {
        try { await stage('playbackPreparation', () => client.playbackReady); }
        catch (error) {
          if (clientPromise === pendingClient) clientPromise = null;
          infoCache.clear();
          throw error;
        }
      }
      let cached = infoCache.get(request.videoId);
      if (request.refresh || (cached && cached.expires <= now())) { infoCache.delete(request.videoId); cached = null; }
      let audio, format, headers, expires, decodeStarted, selectedInfo;
      let retryDecoderCpuMs = 0, retryWrapperCpuMs = 0, retryDecoderMaxRssBytes = 0;
      const extractionStarted = performance.now();
      let refreshedAfterDenial = false;
      for (let attempt = 0; attempt < 2; attempt++) {
        const info = cached?.info ?? await stage('extraction', () => client.getBasicInfo(request.videoId, { client: clientName }));
        selectedInfo = info;
        const actualId = info.basic_info?.id;
        if (actualId !== request.videoId) throw failure('RECORDING_MISMATCH', 'extraction');
        let decodeSeconds = request.seconds ?? 5;
        let minimumDecodedSeconds = decodeSeconds - 0.1;
        if (request.fullRecording === true) {
          const sourceDuration = info.basic_info?.duration;
          if (!Number.isFinite(sourceDuration) || sourceDuration < 5 || sourceDuration > 600 ||
              Math.abs(sourceDuration - request.seconds) > 1) throw failure('RECORDING_MISMATCH', 'extraction');
          decodeSeconds = sourceDuration + 1;
          minimumDecodedSeconds = sourceDuration - 1;
        }
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
          ({ format, headers } = await stage('format', async () => {
            let selected;
            if (request.formatId === undefined) selected = info.chooseFormat({ type: 'audio', quality: 'best', format: 'any' });
            else {
              try { selected = info.chooseFormat({ itag: request.formatId }); }
              catch { throw failure('FORMAT_UNAVAILABLE', 'format'); }
              if (selected?.itag !== request.formatId || selected.has_audio === false) throw failure('FORMAT_MISMATCH', 'format');
            }
            return { format: selected, headers: playbackHeaders(client) };
          }));
          const stream = await stage('decipher', () => format.decipher(client.session.player));
          const parsed = new URL(stream);
          const urlExpiry = Number(parsed.searchParams.get('expire')) * 1000;
          if (urlExpiry > 0) expires = Math.min(expires, urlExpiry - 30_000);
          if (urlExpiry > 0 && urlExpiry <= now() + 30_000) throw failure('MANIFEST_EXPIRED', 'decipher');
          if (expires <= now()) infoCache.delete(request.videoId);
          else if (infoCache.has(request.videoId)) infoCache.get(request.videoId).expires = expires;
          decodeStarted = performance.now();
          audio = await stage('decode', () => decode(stream, { position: request.position ?? 0, seconds: decodeSeconds,
            minimumDecodedSeconds, headers, signal: budget.signal }));
          break;
        } catch (error) {
          infoCache.delete(request.videoId);
          retryDecoderCpuMs += error.decoderUsage?.childCpuMs ?? 0;
          retryWrapperCpuMs += error.decoderUsage?.wrapperCpuMs ?? 0;
          retryDecoderMaxRssBytes = Math.max(retryDecoderMaxRssBytes, error.decoderUsage?.childMaxRssBytes ?? 0);
          if (!['STREAM_DENIED', 'MANIFEST_EXPIRED'].includes(error.code) || attempt === 1) throw error;
          refreshedAfterDenial = true; cached = null;
          if (ensureToken) {
            const refreshed = await stage('token', () => ensureToken(client, { videoId: request.videoId, signal: budget.signal, refresh: true }));
            token = { ...refreshed, tokenHelperCpuMs: (token.tokenHelperCpuMs ?? 0) + (refreshed.tokenHelperCpuMs ?? 0) };
          }
        }
      }
      const used = process.cpuUsage(cpu);
      const helperCpuMs = (used.user + used.system) / 1000;
      const metadata = selectedInfo?.basic_info;
      const providerMetadata = {
        videoId: request.videoId,
        title: typeof metadata?.title === 'string' ? metadata.title.slice(0, 256) : null,
        channel: typeof metadata?.author === 'string' ? metadata.author.slice(0, 256) : null,
        durationSeconds: Number.isFinite(metadata?.duration) ? metadata.duration : null,
      };
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
        wallMs: performance.now() - started,
        formatId: Number.isInteger(format.itag) ? format.itag : null,
        headerNames: Object.keys(headers).sort(),
        playbackPreparation: client.playbackPreparation,
        providerMetadata,
        codec: /^audio\/[\w.+-]+(?:; codecs="[\w., -]+")?$/.test(format.mime_type ?? '') ? format.mime_type : null, recordingVerified: true,
        identityScope: 'explicit-provider-video-id; not acoustic verification' };
    } catch (error) {
      if (budget.signal.aborted) { clientPromise = null; infoCache.clear(); }
      error.stages = stages;
      throw error;
    } finally { budget.close(); }
  };
}
