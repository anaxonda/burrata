#!/usr/bin/env node
const verificationTitle=id=>id===undefined?'Burrata verification':typeof id==='string'&&/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id)?`Burrata verification ${id}`:null;
// Experimental, opt-in YouTube InnerTube worker. It accepts only a consented
// SAPISID-family credential over stdin and never reads browser data. Playlist
// edits require an explicitly approved preview and a fresh exact snapshot.
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import vm from 'node:vm';
import { createHash, webcrypto } from 'node:crypto';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const MAX_INPUT = 1024 * 1024;
const MAX_OUTPUT = 2 * 1024 * 1024;
const MAX_RESPONSE = 2 * 1024 * 1024;
const DEADLINE_MS = 20_000;
// YouTube Music returns its deprecated-browser page for Node's default user agent.
const DESKTOP_USER_AGENT = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36';
const ERROR_PHASES = new Set(['bootstrap', 'bootstrap_config', 'innertube', 'account_identity', 'account_selection', 'channel_identity', 'playlist_library', 'playlist_creation', 'playlist_creation_response', 'worker_deadline']);

const fail = (code, message, phase = undefined, httpStatus = undefined, retryAfterSeconds = undefined) => {
  const safePhase = ERROR_PHASES.has(phase) ? phase : null;
  const safeStatus = Number.isInteger(httpStatus) && httpStatus >= 100 && httpStatus <= 599 ? httpStatus : null;
  const safeRetry = Number.isInteger(retryAfterSeconds) && retryAfterSeconds >= 0 && retryAfterSeconds <= 3600 ? retryAfterSeconds : null;
  // The current Rust transport preserves code/message, so retain fixed diagnostics there too.
  const detail = safePhase ? ` (phase=${safePhase}${safeStatus !== null ? `, HTTP ${safeStatus}` : ''})` : '';
  return { ok: false, error: {
    code, message: message + detail,
    ...(safePhase ? { phase: safePhase } : {}),
    ...(safeStatus !== null ? { httpStatus: safeStatus } : {}),
    ...(safeRetry !== null ? { retryAfterSeconds: safeRetry } : {}),
  } };
};

function cookieSecret(header) {
  if (typeof header !== 'string' || header.length > 16_384) return null;
  const cookies = Object.create(null);
  const allowed = ['SAPISID', '__Secure-1PAPISID', '__Secure-3PAPISID', 'LOGIN_INFO', 'SID', 'HSID', 'SSID', 'APISID', '__Secure-1PSID', '__Secure-3PSID', 'SIDCC', '__Secure-1PSIDTS', '__Secure-3PSIDTS', '__Secure-1PSIDCC', '__Secure-3PSIDCC'];
  for (const [index, part] of header.split(';').entries()) {
    const pair = index === 0 ? part : part.replace(/^[ \t]+/, '');
    const equals = pair.indexOf('=');
    if (equals < 1) return null;
    const name = pair.slice(0, equals), value = pair.slice(equals + 1);
    if (!allowed.includes(name)
        || Object.hasOwn(cookies, name)
        || !/^[^\s;\u0000-\u001f\u007f]{8,4096}$/.test(value)) return null;
    cookies[name] = value;
  }
  return ['SAPISID', '__Secure-1PAPISID', '__Secure-3PAPISID'].some(name => Object.hasOwn(cookies, name))
    ? { values: Object.values(cookies), cookies } : null;
}

function expectedAuthorization(cookies, timestamp, origin) {
  const sources = [
    ['SAPISIDHASH', cookies.SAPISID || cookies['__Secure-3PAPISID']],
    ['SAPISID1PHASH', cookies['__Secure-1PAPISID']],
    ['SAPISID3PHASH', cookies['__Secure-3PAPISID']],
  ].filter(([, value]) => value);
  return sources.map(([scheme, value]) => `${scheme} ${timestamp}_${createHash('sha1').update(`${timestamp} ${value} ${origin}`).digest('hex')}`).join(' ');
}

function allowedUrl(raw, method, origin, operation) {
  let url;
  try { url = new URL(raw); } catch { return false; }
  if (url.username || url.password || url.hash) return false;
  const path = url.pathname;
  const accountList = path === '/youtubei/v1/account/accounts_list';
  const webResolve = path === '/youtubei/v1/navigation/resolve_url';
  const generalLibrary = ['listPlaylists', 'getPlaylist', 'applyPlaylistEdits', 'search', 'createVerificationPlaylist'].includes(operation) && ['/feed/playlists', '/youtubei/v1/browse', '/youtubei/v1/browse/edit_playlist', '/youtubei/v1/search', '/youtubei/v1/playlist/create', '/youtubei/v1/account/account_menu'].includes(path);
  if (url.origin !== (accountList || webResolve || generalLibrary ? 'https://www.youtube.com' : origin)) return false;
  if (path === '/' || path === '/feed/playlists') return method === 'GET' && url.search === '';
  if (method !== 'POST' || !/^\/youtubei\/v1\/(?:browse|browse\/edit_playlist|playlist\/create|search|navigation\/resolve_url|account\/(?:accounts_list|account_menu))$/.test(path)) return false;
  for (const key of url.searchParams.keys()) if (!['key', 'prettyPrint'].includes(key)) return false;
  const apiKey = url.searchParams.get('key');
  return (!apiKey || /^[A-Za-z0-9_-]{10,160}$/.test(apiKey)) && url.searchParams.get('prettyPrint') === 'false';
}

async function limitedResponse(response) {
  const reader = response.body?.getReader();
  if (!reader) {
    const text = await response.text();
    if (Buffer.byteLength(text, 'utf8') > MAX_RESPONSE) throw Object.assign(new Error('oversize'), { code: 'PROVIDER_CHANGED' });
    return new Response(text, { status: response.status, statusText: response.statusText, headers: response.headers });
  }
  let size = 0;
  const chunks = [];
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_RESPONSE) { await reader.cancel(); throw Object.assign(new Error('oversize'), { code: 'PROVIDER_CHANGED' }); }
    chunks.push(value);
  }
  return new Response(new Blob(chunks), { status: response.status, statusText: response.statusText, headers: response.headers });
}

// Creation receipts never contain provider bodies, account IDs, URLs or credentials.
// No receipt on a transport failure means UNKNOWN, never an inferred false dispatch.
export async function executeYouTubeJob(job, options = {}) {
  if (job?.operation !== 'createVerificationPlaylist') return executeYouTubeJobInner(job, options);
  const trace = { mutationDispatched: false, phase: 'worker_start' };
  const requestId = job.creationRequestId;
  if (typeof requestId !== 'string' || !/^[a-f0-9]{32}$/.test(requestId)) return { ok: false, error: { code: 'UNSUPPORTED', message: 'Creation requires a native attempt ID.' } };
  let response;
  try { response = await executeYouTubeJobInner(job, { ...options, creationTrace: trace }); }
  catch { response = { ok: false, error: { code: 'OUTCOME_UNKNOWN', message: 'Creation outcome is unknown. Do not retry creation.' } }; trace.mutationDispatched = null; }
  const phase = /^[a-z_]{1,40}$/.test(response?.error?.phase || '') ? response.error.phase : trace.phase;
  const id = response?.result?.playlistId;
  const validId = /^PL[A-Za-z0-9_-]{10,78}$/.test(typeof id === 'string' ? id : '');
  if (response?.ok === true && (!validId || trace.mutationDispatched !== true || response.result.accountId !== job.args?.accountId || response.result.title !== verificationTitle(job.args?.verificationIntentId))) {
    response = { ok: false, error: { code: 'OUTCOME_UNKNOWN', message: 'Creation receipt could not be verified. Do not retry creation.' } };
  }
  const receipt = { version: 1, requestId, mutationDispatched: trace.mutationDispatched,
    phase: /^[a-z_]{1,40}$/.test(phase || '') ? phase : 'worker_start',
    playlistId: response.ok === true && validId ? id : null };
  if (response.ok === true) response.result.creationReceipt = receipt;
  else response.error.creationReceipt = receipt;
  return response;
}

async function executeYouTubeJobInner(job, { fetchImpl = globalThis.fetch, timeoutMs = DEADLINE_MS, creationTrace = null } = {}) {
  if (!job || job.prototypeEnabled !== true || job.provider !== 'youtube') return fail('UNSUPPORTED', 'The YouTube session prototype is disabled.');
  const credential = cookieSecret(job.session?.cookieHeader);
  if (!credential) return fail('UNSUPPORTED', 'The YouTube session handoff is invalid.');
  const origin = job.session?.origin;
  if (!['https://www.youtube.com', 'https://music.youtube.com'].includes(origin)) return fail('UNSUPPORTED', 'The YouTube session origin is invalid.');
  const dataSyncId = job.session?.dataSyncId;
  const hasDataSyncId = Object.prototype.hasOwnProperty.call(job.session, 'dataSyncId');
  if (hasDataSyncId && (typeof dataSyncId !== 'string' || !/^[A-Za-z0-9_:-]{1,256}$/.test(dataSyncId))) return fail('UNSUPPORTED', 'The YouTube session context is invalid.');
  const deadlineAt = Date.now() + Math.max(1, Math.min(DEADLINE_MS, Number(timeoutMs) || DEADLINE_MS));
  let creationDispatched = false;
  const safeFetch = async (input, options = {}) => {
    const url = new URL(typeof input === 'string' ? input : input.url);
    const method = String(options.method || 'GET').toUpperCase();
    if (!allowedUrl(url.href, method, origin, job.operation)) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    const headers = new Headers(options.headers || {});
    if (headers.get('cookie') !== job.session.cookieHeader) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    const auth = headers.get('authorization') || '';
    const authMatch = /^(?:(?:SAPISIDHASH|SAPISID1PHASH|SAPISID3PHASH) (\d{1,12})_[a-f0-9]{40})(?: (?:(?:SAPISIDHASH|SAPISID1PHASH|SAPISID3PHASH) \1_[a-f0-9]{40}))*$/.exec(auth);
    if (!authMatch || auth !== expectedAuthorization(credential.cookies, authMatch[1], url.origin)) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    if (headers.has('proxy-authorization')) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    const requestOrigin = url.origin;
    if (job.operation === 'listPlaylists' && url.pathname === '/feed/playlists' && (headers.get('x-youtube-client-name') !== '1' || headers.get('x-youtube-client-version') !== '2.20260708.00.00')) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    if (headers.get('origin') !== requestOrigin || headers.get('x-origin') !== requestOrigin || headers.get('referer') !== `${requestOrigin}/`) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    const generalRequest = ['listPlaylists', 'getPlaylist', 'applyPlaylistEdits', 'search', 'createVerificationPlaylist'].includes(job.operation) && requestOrigin === 'https://www.youtube.com';
    if (url.pathname === '/youtubei/v1/playlist/create' && typeof options.body !== 'string') throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    if (typeof options.body === 'string') {
      let requestBody;
      try { requestBody = JSON.parse(options.body); } catch { throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' }); }
      if ((url.pathname.endsWith('/account/accounts_list') || url.pathname.endsWith('/navigation/resolve_url') || generalRequest) && (
        requestBody?.context?.client?.clientName !== 'WEB'
        || requestBody?.context?.client?.clientVersion !== '2.20260708.00.00'
        || headers.get('x-youtube-client-name') !== '1'
        || headers.get('x-youtube-client-version') !== '2.20260708.00.00'
      )) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
      if (url.pathname === '/youtubei/v1/playlist/create') {
        const { context, ...creation } = requestBody;
        if (job.operation !== 'createVerificationPlaylist' || !verificationTitle(job.args?.verificationIntentId) || creationDispatched
            || Object.keys(job.args || {}).length !== (job.args?.verificationIntentId===undefined?1:2) || Object.keys(job.args||{}).some(key=>!['accountId','verificationIntentId'].includes(key)) || job.args.accountId !== job.session.accountId
            || JSON.stringify(creation) !== JSON.stringify({ title: verificationTitle(job.args?.verificationIntentId), privacyStatus: 'PRIVATE', videoIds: [] })) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
      }
      const onBehalfOfUser = requestBody?.context?.user?.onBehalfOfUser;
      if (generalRequest && !url.pathname.endsWith('/account/accounts_list') && !url.pathname.endsWith('/navigation/resolve_url') && onBehalfOfUser !== (dataSyncId || undefined)) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
      if (headers.get('x-goog-pageid') !== (onBehalfOfUser || null)) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
      if (onBehalfOfUser !== undefined && (typeof onBehalfOfUser !== 'string' || !/^[A-Za-z0-9_:-]{1,256}$/.test(onBehalfOfUser))) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
      if ((url.pathname.endsWith('/account/accounts_list') || url.pathname.endsWith('/navigation/resolve_url')) && onBehalfOfUser !== undefined) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
      if (!generalRequest && url.pathname.endsWith('/account/account_menu') && dataSyncId && onBehalfOfUser !== dataSyncId) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
      if (!url.pathname.endsWith('/account/accounts_list') && dataSyncId && onBehalfOfUser !== undefined && onBehalfOfUser !== dataSyncId) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    }
    headers.set('User-Agent', DESKTOP_USER_AGENT);
    if (typeof options.body === 'string' && options.body.length > 64 * 1024) throw Object.assign(new Error('blocked'), { code: 'UNSUPPORTED' });
    const remaining = deadlineAt - Date.now();
    if (remaining <= 0) throw Object.assign(new Error('deadline'), { code: 'RATE_LIMITED', phase: 'worker_deadline' });
    const callerSignal = options.signal;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), Math.max(1, Math.min(DEADLINE_MS, remaining)));
    const abort = () => controller.abort();
    callerSignal?.addEventListener('abort', abort, { once: true });
    try {
      if (url.pathname === '/youtubei/v1/playlist/create') { creationDispatched = true; if (creationTrace) { creationTrace.mutationDispatched = true; creationTrace.phase = 'playlist_creation'; } }
      const response = await fetchImpl(url, { ...options, headers, redirect: 'error', signal: controller.signal, cache: 'no-store' });
      const delay = response.status === 429 && /^\d{1,4}$/.test(response.headers.get('retry-after') || '') ? Math.min(3600, Number(response.headers.get('retry-after'))) : undefined;
      if (response.status === 401 || response.status === 403) throw Object.assign(new Error('YouTube session expired or was denied. Sign in to YouTube again and confirm your channel.'), { code: 'SIGN_IN_REQUIRED', phase: url.pathname === '/youtubei/v1/playlist/create' ? 'playlist_creation' : url.pathname.includes('account/') ? 'account_identity' : 'innertube', httpStatus: response.status });
      if (response.status === 429) throw Object.assign(new Error('YouTube temporarily limited this request. Try again shortly.'), { code: 'RATE_LIMITED', phase: url.pathname === '/youtubei/v1/playlist/create' ? 'playlist_creation' : url.pathname.includes('account/') ? 'account_identity' : 'innertube', httpStatus: 429, retryAfterSeconds: delay });
      if (!response.ok) throw Object.assign(new Error('YouTube returned an unsupported response.'), { code: 'PROVIDER_CHANGED', phase: url.pathname === '/youtubei/v1/playlist/create' ? 'playlist_creation' : url.pathname === '/' ? 'bootstrap' : url.pathname.includes('account/') ? 'account_identity' : 'innertube', httpStatus: response.status });
      return await limitedResponse(response);
    } catch (error) {
      if (controller.signal.aborted) throw Object.assign(new Error('YouTube request timed out.'), { code: 'RATE_LIMITED', phase: url.pathname === '/youtubei/v1/playlist/create' ? 'playlist_creation' : 'innertube' });
      throw error;
    } finally { clearTimeout(timer); callerSignal?.removeEventListener('abort', abort); }
  };
  try {
    const source = await readFile(resolve(ROOT, 'extension/youtube-session-provider.js'), 'utf8');
    const context = vm.createContext({ fetch: safeFetch, URL, URLSearchParams, Response, Headers, AbortController, TextEncoder, Blob, crypto: webcrypto, setTimeout, clearTimeout, Date, Math, Number, String, JSON, Array, Object, RegExp, Map, Set });
    context.globalThis = context;
    vm.runInContext(source, context, { timeout: 1000, filename: 'youtube-session-provider.js' });
    const result = await context.BurrataYouTubeSessionProvider.run(job.operation, job.args || {}, job.session, { fetchImpl: safeFetch, timeoutMs });
    const serialized = JSON.stringify(result);
    if (!serialized || serialized.length > MAX_OUTPUT || credential.values.some(secret => serialized.includes(secret)) || /SAPISID(?:1P|3P)?HASH\s+\d+_[a-f0-9]{40}|(?:cookieHeader|accessToken|authorization|"cookie")\s*:/i.test(serialized)) return fail(creationDispatched ? 'OUTCOME_UNKNOWN' : 'PROVIDER_CHANGED', creationDispatched ? 'Playlist creation may have succeeded. Do not retry creation; inspect the provider library.' : 'YouTube response could not be safely normalized.');
    return { ok: true, result };
  } catch (error) {
    const code = creationDispatched ? 'OUTCOME_UNKNOWN' : ['UNSUPPORTED', 'SIGN_IN_REQUIRED', 'RATE_LIMITED', 'PROVIDER_CHANGED', 'ACCOUNT_CHANGED', 'CONFLICT', 'OUTCOME_UNKNOWN'].includes(error?.code) ? error.code : 'PROVIDER_CHANGED';
    const message = code === 'ACCOUNT_CHANGED' ? 'The active YouTube account changed. Confirm the account again before continuing.'
      : code === 'PROVIDER_CHANGED' && error?.phase === 'channel_identity' ? 'YouTube returned an authenticated account, but Burrata could not verify its active channel ID. Select the intended channel in YouTube Music and retry.'
      : code === 'SIGN_IN_REQUIRED' ? 'YouTube session expired or identity could not be verified. Sign in to YouTube again and confirm your channel.'
        : code === 'RATE_LIMITED' ? 'YouTube temporarily limited this request. Try again shortly.'
          : code === 'OUTCOME_UNKNOWN' ? 'YouTube may have applied some playlist changes. Refresh the playlist before retrying.'
            : code === 'CONFLICT' ? 'The YouTube playlist changed after preview. Review a new sync preview.'
            : code === 'UNSUPPORTED' ? 'This YouTube operation or session is not supported.'
          : 'YouTube returned an unsupported response.';
    const shape = error?.phase === 'account_selection' ? error.diagnostics : null;
    const structures = Array.isArray(shape?.structures) && shape.structures.length <= 12 && shape.structures.every(key => typeof key === 'string' && /^[a-zA-Z]{1,60}(?:Renderer|ViewModel|Action)$/.test(key)) ? ` [structures=${shape.structures.join('|')}]` : '';
    const counts = shape && ['items','selected','renderers','providerError','listShape','menuShape'].every(key => Number.isInteger(shape[key]) && shape[key] >= 0 && shape[key] <= 599)
      ? ` [items=${shape.items}, selected=${shape.selected}, renderers=${shape.renderers}, loggedOut=${shape.loggedOut === true}, bootstrapLoggedIn=${['true','false','unknown'].includes(shape.bootstrapLoggedIn) ? shape.bootstrapLoggedIn : 'unknown'}, providerError=${shape.providerError}, listShape=${shape.listShape}, menuShape=${shape.menuShape}]` : '';
    const identityShape = error?.phase === 'channel_identity' ? error.diagnostics?.shape : null;
    const identityKeys = ['activeHeader','headerHandle','selectedHandle','pageIdToken','directBrowseId','urlEndpoint','secondResolve'];
    const identitySafe = identityShape && Object.keys(identityShape).length === identityKeys.length
      && identityKeys.every(key => Object.hasOwn(identityShape, key) && typeof identityShape[key] === 'boolean');
    const identityDetail = identitySafe ? ` [channelIdentity ${identityKeys.map(key => `${key}=${identityShape[key]}`).join(' ')}]` : '';
    return fail(code, message + counts + structures + identityDetail, ERROR_PHASES.has(error?.phase) ? error.phase : undefined, error?.httpStatus, error?.retryAfterSeconds);
  }
}

async function main() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += chunk.length;
    if (size > MAX_INPUT) { process.stdout.write(`${JSON.stringify(fail('UNSUPPORTED', 'The YouTube worker request is too large.'))}\n`); return; }
    chunks.push(chunk);
  }
  let job;
  try { job = JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch { process.stdout.write(`${JSON.stringify(fail('UNSUPPORTED', 'The YouTube worker request is invalid.'))}\n`); return; }
  const result = await executeYouTubeJob(job);
  const output = JSON.stringify(result);
  process.stdout.write(`${output.length <= MAX_OUTPUT ? output : JSON.stringify(fail('PROVIDER_CHANGED', 'YouTube response exceeded the output limit.'))}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) void main();
