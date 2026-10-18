import { spawn } from 'node:child_process';
import { performance } from 'node:perf_hooks';

export async function decodePcm(input, { position = 0, seconds = 5, ffmpeg = 'ffmpeg', signal } = {}) {
  const started = performance.now();
  const args = ['-nostdin', '-hide_banner', '-loglevel', 'error', '-rw_timeout', '10000000'];
  if (position) args.push('-ss', String(position));
  args.push('-i', input, '-t', String(seconds), '-vn', '-f', 'f32le', '-acodec', 'pcm_f32le', '-ar', '48000', '-ac', '2', 'pipe:1');
  const child = spawn(ffmpeg, args, { stdio: ['ignore', 'pipe', 'pipe'], signal });
  let bytes = 0, firstPcmMs = null, denied = false;
  child.stdout.on('data', chunk => { firstPcmMs ??= performance.now() - started; bytes += chunk.length; });
  // Only retain an error category; stream URLs and tokens must never reach logs.
  child.stderr.on('data', chunk => { if (/403|410/.test(chunk.toString())) denied = true; });
  await new Promise((resolve, reject) => {
    let failure;
    child.once('error', error => { failure = error; });
    child.once('close', code => failure ? reject(failure) : code === 0 ? resolve() : reject(Object.assign(new Error(), { code: denied ? 'STREAM_DENIED' : 'DECODE_FAILED' })));
  });
  const decodedFrames = Math.floor(bytes / 8);
  if (!decodedFrames) throw Object.assign(new Error(), { code: 'NO_PCM' });
  if (decodedFrames < (seconds - 0.1) * 48000) throw Object.assign(new Error(), { code: 'SHORT_AUDIO' });
  return { firstPcmMs, decodedFrames, decodedAudioMs: decodedFrames / 48, decodeWallMs: performance.now() - started };
}

async function atStage(stage, operation) {
  try { return await operation(); }
  catch (error) { error.benchmarkStage = stage; throw error; }
}

export function createPrototype({ createClient, decode = decodePcm, now = Date.now, clientName = 'WEB' }) {
  let clientPromise;
  const infoCache = new Map();
  return async function execute(request, { signal } = {}) {
    if (!['search', 'play', 'seek'].includes(request?.action)) throw Object.assign(new Error(), { code: 'INVALID_ACTION' });
    if (request.action === 'search') {
      if (typeof request.query !== 'string' || !request.query.trim() || request.query.length > 500) throw Object.assign(new Error(), { code: 'INVALID_QUERY' });
    } else if (!/^[A-Za-z0-9_-]{11}$/.test(request.videoId ?? '') ||
               !Number.isFinite(request.position ?? 0) || (request.position ?? 0) < 0 ||
               !Number.isFinite(request.seconds ?? 5) || (request.seconds ?? 5) < 5 || (request.seconds ?? 5) > 30) {
      throw Object.assign(new Error(), { code: 'INVALID_PLAYBACK_REQUEST' });
    }
    const started = performance.now();
    const cpu = process.cpuUsage();
    clientPromise ??= atStage('session', createClient).catch(error => { clientPromise = null; throw error; });
    const client = await clientPromise;
    const preparedMs = performance.now() - started;
    if (request.action === 'search') {
      const result = await atStage('search', () => client.search(request.query, { type: 'video' }));
      const candidates = (result.results ?? []).filter(row => row.type === 'Video' && row.id).slice(0, 10)
        .map(row => ({ videoId: row.id, title: String(row.title ?? ''), artist: String(row.author?.name ?? ''), durationSeconds: row.duration?.seconds ?? null }));
      return { preparedMs, searchMs: performance.now() - started - preparedMs, candidates };
    }
    let cached = infoCache.get(request.videoId);
    if (cached && cached.expires <= now()) { infoCache.delete(request.videoId); cached = null; }
    const extractionStarted = performance.now();
    const info = cached?.info ?? await atStage('extraction', () => client.getBasicInfo(request.videoId, { client: clientName }));
    if (!cached) {
      const expiry = info.streaming_data?.expires instanceof Date ? info.streaming_data.expires.getTime() - 30_000 : now() + 60_000;
      if (infoCache.size >= 8) infoCache.delete(infoCache.keys().next().value);
      infoCache.set(request.videoId, { info, expires: Math.min(expiry, now() + 300_000) });
    }
    let format, stream;
    try {
      format = await atStage('format', async () => info.chooseFormat({ type: 'audio', quality: 'best', format: 'any' }));
      stream = await atStage('decipher', () => format.decipher(client.session.player));
    } catch (error) { infoCache.delete(request.videoId); throw error; }
    const extractionMs = performance.now() - extractionStarted;
    let audio;
    try {
      audio = await atStage('decode', () => decode(stream, { position: request.position ?? 0, seconds: request.seconds ?? 5, signal }));
    } catch (error) {
      infoCache.delete(request.videoId);
      throw error;
    }
    const used = process.cpuUsage(cpu);
    return { clientName, preparedMs, extractionMs, infoCacheHit: Boolean(cached), ...audio,
      helperCpuMs: (used.user + used.system) / 1000, helperRssBytes: process.memoryUsage().rss,
      wallMs: performance.now() - started, codec: format.mime_type ?? null,
      recordingVerified: request.expectedVideoId === request.videoId };
  };
}
