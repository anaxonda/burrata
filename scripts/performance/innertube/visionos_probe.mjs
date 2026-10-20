// Diagnostic-only VISIONOS_0_1 capability probe. Never imported by production.
// Pin: InnerTubeX v0.5.2 / 93b650b877530738cbc4d2183b4322b77bb9033e.
// Default mode reproduces the anonymous nullable-visitor profile; explicit
// anonymous-source mode follows its source visitor lookup without retaining a
// session. No watch/player-JS/token fetch occurs. Callers must retain a
// separate process-group wall deadline; the probe's absolute budget is <=20s.
import { randomBytes } from 'node:crypto';
import { performance } from 'node:perf_hooks';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { decodePcm } from './core.mjs';
import { commandBudget, failure } from './budget.mjs';

export const PROFILE = Object.freeze({
  id: 'VISIONOS_0_1', clientName: 'VISIONOS', clientVersion: '0.1', clientId: '101',
  endpoint: 'https://music.youtube.com/youtubei/v1/player?prettyPrint=false',
  userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15',
});
export const CONSTRUCTION_VERSION = 'innertubex-93b-profile-meld-9c3-defaults-v2';
export const VISITOR_CONSTRUCTION_VERSION = 'innertubex-93b-profile-meld-9c3-anonymous-visitor-v3';
const SOURCE_PINS = Object.freeze({
  meldHeaderDefaults: '9c3dfc8618625a94e7019fbb0f3e31f2c87f74e9',
  meldOriginalComparison: '1ded7122ee0bd166bd703bfe3b63d233af8b7dcd',
  innertubex: '93b650b877530738cbc4d2183b4322b77bb9033e',
  innertubexArchiveSha256: 'ba9519a7851131aa93094c4b5a0dfb1ee238f8a0975a85274c9892cb412cbcc2',
  clientModelSha256: '694d66a4c8dc4f9c67b1abc971835ddbd5d09022de8b286a986ad5039b846d90',
  playbackCatalogSha256: 'ab204b0ee78c87d4c7e9f4e82171d6c399b2013bd08942f4dae6846611e3af54',
});
const MAX_BODY = 4 * 1024 * 1024;
const VALID_ID = /^[A-Za-z0-9_-]{11}$/;
const ERROR_CODES = new Set(['INVALID_PLAYBACK_REQUEST', 'INVALID_BUDGET', 'INVALID_JSON',
  'TIMEOUT', 'CANCELLED', 'NETWORK_FAILED', 'PLAYER_HTTP_ERROR', 'RESPONSE_TOO_LARGE',
  'INVALID_RESPONSE', 'PLAYABILITY_REJECTED', 'RECORDING_MISMATCH', 'METADATA_MISSING', 'METADATA_MISMATCH',
  'FORMAT_UNAVAILABLE', 'FORMAT_MISMATCH', 'CIPHER_REQUIRED', 'N_TRANSFORM_REQUIRED',
  'UNSUPPORTED_TRANSPORT', 'INVALID_MEDIA_URL', 'FORMAT_LENGTH_MISSING',
  'FORMAT_DURATION_MISSING', 'PREVIEW_OR_TRUNCATED_FORMAT', 'MANIFEST_EXPIRED',
  'DECODE_FAILED', 'DECODER_UNAVAILABLE', 'STREAM_DENIED', 'NO_PCM', 'SHORT_AUDIO', 'VISITOR_CONTEXT_UNAVAILABLE',
  'INVALID_DECODE_RANGE', 'FULL_RECORDING_TOO_LONG', 'TRUNCATED_RECORDING', 'FULL_RECORDING_DURATION_MISMATCH']);
const STAGES = new Set(['request', 'visitorServiceWorker', 'visitorHomepage', 'player', 'parse', 'format', 'decode']);

export function buildVisionRequest(request, { visitorData } = {}) {
  const allowed = new Set(['id', 'videoId', 'expectedVideoId', 'formatId', 'budgetMs', 'hl', 'gl', 'expectedMetadata',
    'visitorMode', 'position', 'positionFraction', 'seconds', 'fullRecording']);
  if (!request || typeof request !== 'object' || Array.isArray(request)
    || Object.keys(request).some(key => !allowed.has(key))
    || !VALID_ID.test(request.videoId ?? '') || request.expectedVideoId !== request.videoId
    || (request.id !== undefined && !/^[A-Za-z0-9_-]{1,64}$/.test(request.id))
    || !/^[a-z]{2,3}(?:[-_][A-Za-z0-9]{2,8})*$/.test(request.hl ?? '')
    || !/^[A-Z]{2}$/.test(request.gl ?? '')
    || !['omit', 'anonymous-source'].includes(request.visitorMode ?? 'omit')
    || !Number.isFinite(request.position ?? 0) || (request.position ?? 0) < 0
    || (request.position !== undefined && request.positionFraction !== undefined)
    || (request.positionFraction !== undefined && (!Number.isFinite(request.positionFraction)
      || request.positionFraction < 0 || request.positionFraction >= 1))
    || !Number.isFinite(request.seconds ?? 5) || (request.seconds ?? 5) < 0.1 || (request.seconds ?? 5) > 600
    || (request.fullRecording !== undefined && typeof request.fullRecording !== 'boolean')
    || (request.fullRecording === true && (!request.expectedMetadata || (request.position ?? 0) !== 0
      || (request.positionFraction ?? 0) !== 0 || request.seconds !== undefined
      || request.expectedMetadata.durationSeconds > 600))
    || !Number.isInteger(request.formatId ?? 251) || (request.formatId ?? 251) < 1
    || (request.formatId ?? 251) > 999_999) throw failure('INVALID_PLAYBACK_REQUEST', 'request');
  if (visitorData !== undefined && (request.visitorMode !== 'anonymous-source' || !validVisitor(visitorData))) {
    throw failure('INVALID_PLAYBACK_REQUEST', 'request');
  }
  if (request.expectedMetadata !== undefined) {
    const metadata = request.expectedMetadata;
    if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)
      || Object.keys(metadata).some(key => !['titleNeedle', 'channelNeedle', 'durationSeconds'].includes(key))
      || !['titleNeedle', 'channelNeedle'].every(key => typeof metadata[key] === 'string'
        && normalized(metadata[key]) && metadata[key].length <= 256)
      || !Number.isFinite(metadata.durationSeconds) || metadata.durationSeconds < 5
      || metadata.durationSeconds > 86_400) throw failure('INVALID_PLAYBACK_REQUEST', 'request');
  }
  const budgetMs = request.budgetMs ?? 19_000;
  if (!Number.isInteger(budgetMs) || budgetMs < 1 || budgetMs > 20_000) throw failure('INVALID_BUDGET', 'request');
  const languageTag = request.hl.replaceAll('_', '-');
  const regionalTag = languageTag.includes('-') ? languageTag : `${languageTag}-${request.gl}`;
  return {
    endpoint: PROFILE.endpoint,
    budgetMs,
    formatId: request.formatId ?? 251,
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      'Cache-Control': 'no-cache',
      'X-Goog-Api-Format-Version': '1',
      'X-YouTube-Client-Name': PROFILE.clientId,
      'X-YouTube-Client-Version': PROFILE.clientVersion,
      Origin: 'https://music.youtube.com',
      'X-Origin': 'https://music.youtube.com',
      Referer: 'https://music.youtube.com/',
      'Accept-Language': `${regionalTag},${languageTag.split('-')[0]};q=0.9`,
      'User-Agent': PROFILE.userAgent,
      ...(visitorData === undefined ? {} : { 'X-Goog-Visitor-Id': visitorData }),
    },
    body: {
      context: {
        client: { clientName: PROFILE.clientName, clientVersion: PROFILE.clientVersion,
          osName: 'VISION_OS', osVersion: '1.3', deviceMake: 'Apple', deviceModel: 'RealityDevice14,1',
          platform: 'MOBILE', gl: request.gl, hl: request.hl,
          ...(visitorData === undefined ? {} : { visitorData }) },
        request: { internalExperimentFlags: [], useSsl: true },
        user: { lockedSafetyMode: false },
      },
      videoId: request.videoId,
      contentCheckOk: true,
      racyCheckOk: true,
    },
  };
}

// These are source-defined anonymous context lookups, not a token or account
// bootstrap. A JSONL caller cannot inject a visitor identifier or credentials.
export function buildVisitorRequests(request) {
  const player = buildVisionRequest(request);
  const headers = {
    'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0',
    'Accept-Language': player.headers['Accept-Language'],
    'Cache-Control': 'no-cache',
  };
  return [
    { source: 'service-worker', stage: 'visitorServiceWorker', endpoint: 'https://www.youtube.com/sw.js_data',
      headers: { ...headers, Accept: 'application/json,text/plain,*/*' } },
    { source: 'music-homepage', stage: 'visitorHomepage', endpoint: 'https://music.youtube.com',
      headers: { ...headers, Accept: 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8' } },
  ];
}

function validVisitor(value) {
  return typeof value === 'string' && value.length > 0 && value.length <= 4096 && /^[\x21-\x7e]+$/.test(value);
}

export function parseAnonymousVisitor(text, source) {
  let value;
  if (source === 'service-worker') {
    try {
      // Kotlin substringAfter('\n') returns the original string if absent.
      const payload = JSON.parse(text.slice(text.indexOf('\n') + 1).trimStart());
      value = payload?.[0]?.[2]?.[0]?.[0]?.[13];
    } catch { return null; }
  } else if (source === 'music-homepage') {
    value = /"(?:VISITOR_DATA|visitorData)"\s*:\s*"([^"]+)"/.exec(text)?.[1];
  }
  return validVisitor(value) ? value : null;
}

function numeric(value) {
  if (typeof value !== 'number' && (typeof value !== 'string' || !/^\d+(?:\.\d+)?$/.test(value))) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : null;
}
function codec(value) {
  return typeof value === 'string' && value.length <= 128
    && /^audio\/[\w.+-]+(?:;\s*codecs="[\w., -]+")?$/.test(value) ? value : null;
}
function normalized(value) {
  return value.normalize('NFKC').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
}
function needsN(url) {
  return /(?:[?&]|%26)n(?:=|%3d)/i.test(url) || new URL(url).searchParams.has('n');
}
function mediaUrl(value) {
  let url;
  try { url = new URL(value); } catch { throw failure('INVALID_MEDIA_URL', 'format'); }
  if (url.protocol !== 'https:' || (url.port && url.port !== '443') || url.username || url.password
    || url.hash || url.pathname !== '/videoplayback'
    || !['googlevideo.com', 'youtube.com'].some(host => url.hostname === host || url.hostname.endsWith(`.${host}`))) {
    throw failure('INVALID_MEDIA_URL', 'format');
  }
  return url;
}
function formatInventory(body) {
  const data = body?.streamingData;
  return [...(Array.isArray(data?.formats) ? data.formats : []), ...(Array.isArray(data?.adaptiveFormats) ? data.adaptiveFormats : [])]
    .filter(format => Number.isInteger(format?.itag) && format.itag > 0 && codec(format.mimeType))
    .slice(0, 32).map(format => ({ formatId: format.itag, codec: codec(format.mimeType),
      directUrlPresent: typeof format.url === 'string' && Boolean(format.url),
      cipherPresent: Boolean(format.signatureCipher || format.cipher),
      contentLength: numeric(format.contentLength), approximateDurationMs: numeric(format.approxDurationMs) }));
}

// Conservative categories from the pinned PlayabilityFailure.kt status/reason
// predicates. The response text is inspected only in memory and never emitted.
// These categories cannot distinguish a region from an unsupported client.
export function playabilityCategory(body) {
  const status = body?.playabilityStatus?.status;
  const reason = typeof body?.playabilityStatus?.reason === 'string'
    ? body.playabilityStatus.reason.toLowerCase() : '';
  if (['AGE_CHECK_REQUIRED', 'AGE_VERIFICATION_REQUIRED', 'CONTENT_CHECK_REQUIRED'].includes(status)
    || ['confirm your age', 'age-restricted', 'age restricted', 'age verification'].some(text => reason.includes(text))) {
    return 'AGE_RESTRICTED';
  }
  if (status === 'LOGIN_REQUIRED') return 'LOGIN_REQUIRED';
  if (['video unavailable', 'not available', 'private video', 'has been removed'].some(text => reason.includes(text))) {
    return 'UNAVAILABLE';
  }
  if (status === 'OK') return 'OK';
  return 'UNCLASSIFIED_REJECTION';
}

// Inspect codes only; never inspect or emit error text, stacks, hostnames or URLs.
export function networkCause(error) {
  const allowed = new Set(['ECONNRESET', 'UND_ERR_SOCKET', 'ETIMEDOUT', 'ENOTFOUND', 'EAI_AGAIN']);
  const pending = [error], seen = new Set();
  for (let count = 0; pending.length && count < 16; count++) {
    const value = pending.shift();
    if (!value || typeof value !== 'object' || seen.has(value)) continue;
    seen.add(value);
    if (allowed.has(value.code)) return value.code;
    if (value.cause) pending.push(value.cause);
    if (Array.isArray(value.errors)) pending.push(...value.errors.slice(0, 4));
  }
  return 'OTHER';
}

export function selectVisionAudio(body, request, now = Date.now()) {
  const actualId = body?.videoDetails?.videoId;
  if (!VALID_ID.test(actualId ?? '') || actualId !== request.videoId) throw failure('RECORDING_MISMATCH', 'parse');
  const metadata = body.videoDetails;
  const duration = numeric(metadata.lengthSeconds);
  if (typeof metadata.title !== 'string' || !metadata.title.trim()
    || typeof metadata.author !== 'string' || !metadata.author.trim() || !duration || duration < 5
    || metadata.isLiveContent === true) throw failure('METADATA_MISSING', 'parse');
  const expected = request.expectedMetadata;
  if (expected && (!normalized(metadata.title).includes(normalized(expected.titleNeedle))
    || !normalized(metadata.author).includes(normalized(expected.channelNeedle))
    || Math.abs(duration - expected.durationSeconds) > 1)) throw failure('METADATA_MISMATCH', 'parse');
  // Upstream's 0.1 profile skips response status validation. This diagnostic is
  // deliberately stricter: an OK response and explicit matching ID are required.
  if (body?.playabilityStatus?.status !== 'OK') throw failure('PLAYABILITY_REJECTED', 'parse');
  const data = body.streamingData;
  const formats = [...(Array.isArray(data?.formats) ? data.formats : []), ...(Array.isArray(data?.adaptiveFormats) ? data.adaptiveFormats : [])];
  const candidates = formats.filter(format => format?.itag === (request.formatId ?? 251));
  if (!candidates.length) throw failure('FORMAT_UNAVAILABLE', 'format');
  if (candidates.length !== 1 || !codec(candidates[0].mimeType) || candidates[0].width !== undefined
    || candidates[0].height !== undefined || candidates[0].hasAudio === false) throw failure('FORMAT_MISMATCH', 'format');
  const format = candidates[0];
  if (format.signatureCipher || format.cipher) throw failure('CIPHER_REQUIRED', 'format');
  if (!format.url) throw failure('UNSUPPORTED_TRANSPORT', 'format');
  const url = mediaUrl(format.url);
  if (needsN(format.url)) throw failure('N_TRANSFORM_REQUIRED', 'format');
  const length = numeric(format.contentLength);
  if (!Number.isSafeInteger(length) || length <= 0) throw failure('FORMAT_LENGTH_MISSING', 'format');
  const approximateDurationMs = numeric(format.approxDurationMs);
  if (!approximateDurationMs) throw failure('FORMAT_DURATION_MISSING', 'format');
  if (approximateDurationMs < duration * 1000 - Math.max(1000, duration * 10)) {
    throw failure('PREVIEW_OR_TRUNCATED_FORMAT', 'format');
  }
  const urlExpiry = numeric(url.searchParams.get('expire'));
  const responseExpiry = numeric(data?.expiresInSeconds);
  const deadlines = [];
  if (urlExpiry) deadlines.push(urlExpiry * 1000);
  if (responseExpiry) deadlines.push(now + responseExpiry * 1000);
  const expires = deadlines.length ? Math.min(...deadlines) : null;
  if (!expires || expires <= now + 30_000) throw failure('MANIFEST_EXPIRED', 'format');
  // Match InnerTubeX's direct-URL handoff. Keep the nonce ephemeral and unreported.
  if (!url.searchParams.has('cpn')) url.searchParams.set('cpn', randomBytes(12).toString('base64url'));
  return { url: url.href, headers: {}, formatId: format.itag, codec: codec(format.mimeType),
    expiresInMs: expires - now, contentLength: length, approximateDurationMs,
    metadata: { videoId: actualId, titlePresent: true, authorPresent: true, durationSeconds: duration,
      matchesExpectedMetadata: expected ? true : null } };
}

export function diagnosticDecodePlan(request, selected) {
  const duration = selected.metadata.durationSeconds;
  const position = request.positionFraction === undefined ? (request.position ?? 0) : duration * request.positionFraction;
  if (request.fullRecording === true) {
    if (duration > 600) throw failure('FULL_RECORDING_TOO_LONG', 'decode');
    return { position: 0, seconds: duration + 1, minimumDecodedSeconds: duration - 1,
      expectedSeconds: duration, maximumDecodedSeconds: duration + 1, fullRecording: true };
  }
  const seconds = request.seconds ?? 5;
  if (position >= duration || position + seconds > duration + 0.001) throw failure('INVALID_DECODE_RANGE', 'decode');
  return { position, seconds, expectedSeconds: seconds, minimumDecodedSeconds: Math.max(0, seconds - 0.1), fullRecording: false };
}

async function readText(response, signal, stage = 'parse') {
  if (numeric(response.headers.get('content-length')) > MAX_BODY) {
    await response.body?.cancel();
    throw failure('RESPONSE_TOO_LARGE', stage);
  }
  const reader = response.body?.getReader();
  if (!reader) throw failure('INVALID_RESPONSE', stage);
  const chunks = [];
  let size = 0;
  try {
    while (true) {
      signal.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > MAX_BODY) throw failure('RESPONSE_TOO_LARGE', stage);
      chunks.push(value);
    }
    return Buffer.concat(chunks).toString('utf8');
  } finally {
    await reader.cancel().catch(() => {});
  }
}

async function readJson(response, signal) {
  const text = await readText(response, signal);
  try { return JSON.parse(text); }
  catch { throw failure('INVALID_RESPONSE', 'parse'); }
}

async function fetchAnonymousVisitor(request, { fetchImpl, budget, stage, report }) {
  report.visitorAttempts = [];
  for (const spec of buildVisitorRequests(request)) {
    const attempt = { source: spec.source, headerNames: Object.keys(spec.headers).sort(), found: false };
    report.visitorAttempts.push(attempt);
    try {
      const value = await stage(spec.stage, async () => {
        const response = await fetchImpl(spec.endpoint, { method: 'GET', headers: spec.headers,
          redirect: 'error', credentials: 'omit', signal: budget.signal });
        attempt.httpStatus = response.status;
        if (!response.ok) {
          await response.body?.cancel();
          return null;
        }
        return parseAnonymousVisitor(await readText(response, budget.signal, spec.stage), spec.source);
      });
      if (value !== null) {
        attempt.found = true;
        report.visitorContext = `anonymous-source-${spec.source}; ephemeral-no-session-cache`;
        return value;
      }
    } catch (error) {
      // Match the source's fallback after a failed service-worker lookup, but
      // never turn cancellation/deadline into a homepage or player request.
      if (budget.signal.aborted) throw error;
      attempt.failure = error?.code === 'RESPONSE_TOO_LARGE' ? 'RESPONSE_TOO_LARGE' : 'CONTEXT_REQUEST_FAILED';
      if (attempt.failure === 'CONTEXT_REQUEST_FAILED') attempt.networkCause = networkCause(error);
    }
  }
  throw failure('VISITOR_CONTEXT_UNAVAILABLE', 'visitorHomepage');
}

export async function runVisionosProbe(request, { fetchImpl = fetch, decode = decodePcm, signal,
  ffmpeg = process.env.BURRATA_BENCH_FFMPEG ?? 'ffmpeg', started = performance.now() } = {}) {
  const startedCpu = process.cpuUsage();
  const report = { schemaVersion: 1, qualification: false, profileId: PROFILE.id,
    constructionVersion: request?.visitorMode === 'anonymous-source' ? VISITOR_CONSTRUCTION_VERSION : CONSTRUCTION_VERSION,
    transportParity: false,
    comparisonScope: 'pinned InnerTubeX profile plus separately pinned Meld application defaults; not full Meld transport or lifecycle',
    sourcePins: SOURCE_PINS, tokenWork: 'not-requested', playerJavaScript: 'not-requested',
    visitorContext: 'omitted-nullable-profile; no-session-fetch',
    scope: 'anonymous explicit provider ID; five-second PCM excerpt; not full-recording, acoustic, catalog or device qualification',
    stages: {} };
  let budget;
  const stage = async (name, operation) => {
    const at = performance.now();
    report.stage = name;
    try { return await budget.run(operation); }
    finally { report.stages[name] = performance.now() - at; }
  };
  try {
    let spec = buildVisionRequest(request);
    if (request.id) report.id = request.id;
    report.requestedVideoId = request.videoId;
    report.requestedFormatId = spec.formatId;
    report.requestHeaderNames = Object.keys(spec.headers).sort();
    budget = commandBudget(spec.budgetMs, { signal, started });
    if (request.visitorMode === 'anonymous-source') {
      report.visitorContext = 'anonymous-source-pending';
      const visitorData = await fetchAnonymousVisitor(request, { fetchImpl, budget, stage, report });
      spec = buildVisionRequest(request, { visitorData });
      report.requestHeaderNames = Object.keys(spec.headers).sort();
    }
    const response = await stage('player', () => fetchImpl(spec.endpoint, {
      method: 'POST', headers: spec.headers, body: JSON.stringify(spec.body),
      redirect: 'error', credentials: 'omit', signal: budget.signal,
    }));
    report.httpStatus = response.status;
    if (!response.ok) {
      await response.body?.cancel();
      throw failure('PLAYER_HTTP_ERROR', 'player');
    }
    const body = await stage('parse', () => readJson(response, budget.signal));
    report.playabilityStatus = ['OK', 'UNPLAYABLE', 'LOGIN_REQUIRED', 'ERROR', 'LIVE_STREAM_OFFLINE'].includes(body?.playabilityStatus?.status)
      ? body.playabilityStatus.status : 'UNKNOWN';
    report.playabilityCategory = playabilityCategory(body);
    report.availableFormats = formatInventory(body);
    const selected = await stage('format', () => selectVisionAudio(body, request));
    const plan = diagnosticDecodePlan(request, selected);
    Object.assign(report, { returnedVideoId: selected.metadata.videoId, metadata: selected.metadata,
      formatId: selected.formatId, codec: selected.codec, expiresInMs: selected.expiresInMs,
      contentLength: selected.contentLength, approximateDurationMs: selected.approximateDurationMs,
      mediaHeaderNames: Object.keys(selected.headers), identityMatchesRequestedId: true,
      positionSeconds: plan.position, expectedDecodeSeconds: plan.expectedSeconds,
      decoderLimitSeconds: plan.seconds, fullRecording: plan.fullRecording,
      minimumDecodedFrames: plan.minimumDecodedSeconds * 48000 });
    report.scope = plan.fullRecording
      ? 'anonymous explicit provider ID; duration-counted PCM within one second of metadata; not acoustic, catalog or device qualification'
      : 'anonymous explicit provider ID; bounded PCM excerpt; not full-recording, acoustic, catalog or device qualification';
    const decodeStarted = performance.now();
    const audio = await stage('decode', () => decode(selected.url, {
      seconds: plan.seconds, position: plan.position, headers: selected.headers, signal: budget.signal, ffmpeg,
      ...(plan.fullRecording ? { minimumDecodedSeconds: plan.minimumDecodedSeconds } : {}),
    }));
    if (!Number.isSafeInteger(audio?.decodedFrames) || audio.decodedFrames <= 0
      || audio.decodedFrames < plan.minimumDecodedSeconds * 48000) {
      throw failure(plan.fullRecording ? 'TRUNCATED_RECORDING' : 'SHORT_AUDIO', 'decode');
    }
    if (plan.fullRecording && audio.decodedFrames > plan.maximumDecodedSeconds * 48000) {
      throw failure('FULL_RECORDING_DURATION_MISMATCH', 'decode');
    }
    report.fullRecordingComplete = plan.fullRecording;
    report.decodedAudioMs = audio.decodedFrames / 48;
    for (const name of ['firstPcmMs', 'decodedFrames', 'decodeWallMs', 'decoderCpuMs',
      'decoderMaxRssBytes', 'accountingWrapperCpuMs', 'accountingWrapperMaxRssBytes']) {
      if (Number.isFinite(audio[name])) report[name] = audio[name];
    }
    report.firstPcmFromCommandMs = Number.isFinite(audio.firstPcmMs) ? decodeStarted - started + audio.firstPcmMs : null;
    report.ok = true;
  } catch (error) {
    const cause = budget?.signal.aborted ? budget.signal.reason : error;
    report.ok = false;
    report.failure = ERROR_CODES.has(cause?.code) ? cause.code : 'NETWORK_FAILED';
    if (report.failure === 'NETWORK_FAILED') report.networkCause = networkCause(cause);
    if (request?.fullRecording === true && ['SHORT_AUDIO', 'NO_PCM'].includes(report.failure)) report.failure = 'TRUNCATED_RECORDING';
    if (STAGES.has(error?.benchmarkStage)) report.stage = error.benchmarkStage;
    if (!STAGES.has(report.stage)) report.stage = 'request';
  } finally {
    budget?.close();
    report.wallMs = performance.now() - started;
    const usedCpu = process.cpuUsage(startedCpu);
    report.helperCpuMs = (usedCpu.user + usedCpu.system) / 1000;
    report.helperRssBytes = process.memoryUsage().rss;
    report.helperMaxRssBytes = process.resourceUsage().maxRSS * 1024;
    report.accountingComplete = ['decoderCpuMs', 'decoderMaxRssBytes', 'accountingWrapperCpuMs', 'accountingWrapperMaxRssBytes']
      .every(name => Number.isFinite(report[name]));
    if (report.accountingComplete) report.totalCpuMs = report.helperCpuMs + report.decoderCpuMs + report.accountingWrapperCpuMs;
  }
  return report;
}

async function main() {
  let input = '';
  for await (const chunk of process.stdin) {
    input += chunk.toString();
    if (Buffer.byteLength(input) > 16_384) throw failure('INVALID_JSON', 'request');
    if (input.includes('\n')) break;
  }
  process.stdin.destroy();
  let request;
  try { request = JSON.parse(input.trim()); } catch { throw failure('INVALID_JSON', 'request'); }
  const controller = new AbortController();
  const stop = () => controller.abort(failure('CANCELLED', 'request'));
  process.once('SIGTERM', stop);
  process.once('SIGINT', stop);
  const result = await runVisionosProbe(request, { signal: controller.signal });
  process.removeListener('SIGTERM', stop);
  process.removeListener('SIGINT', stop);
  process.stdout.write(`${JSON.stringify(result)}\n`);
  if (!result.ok) process.exitCode = 1;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(() => {
    process.stdout.write(`${JSON.stringify({ schemaVersion: 1, qualification: false, profileId: PROFILE.id,
      ok: false, failure: 'INVALID_JSON', stage: 'request' })}\n`);
    process.exitCode = 1;
  });
}
