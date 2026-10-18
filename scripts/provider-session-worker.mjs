#!/usr/bin/env node
const verificationTitle=id=>id===undefined?'Burrata verification':typeof id==='string'&&/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id)?`Burrata verification ${id}`:null;
// Experimental, one-request Spotify read worker. Input arrives only on stdin;
// credentials and provider responses are never written to logs.
import { readFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve } from 'node:path';
import vm from 'node:vm';
import { webcrypto } from 'node:crypto';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const MAX_INPUT = 1024 * 1024;
const MAX_OUTPUT = 2 * 1024 * 1024;
const MAX_RESPONSE = 2 * 1024 * 1024;
const DEADLINE_MS = 20_000;
const CATALOG_GRAPHQL_OPERATIONS = new Set(['queryArtistMinimal', 'queryArtistDiscographyAll', 'getAlbum', 'queryAlbumTracks', 'getListsContents']);
const REGISTRY_URL = 'https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef';
const HASH_REGISTRY_URL = 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json';
const HASHES = {
  searchUsers: '8f358dd82e62f61dd4ceaa9f8cd0889e644c9b707f1b724fbfb356a757cb7e5a',
  profileAttributes: '53bcb064f6cd18c23f752bc324a791194d20df612d8e1239c735144ab0399ced',
  libraryV3: '973e511ca44261fda7eebac8b653155e7caee3675abb4fb110cc1b8c78b091c3',
  fetchPlaylist: '346811f856fb0b7e4f6c59f8ebea78dd081c6e2fb01b77c954b26259d5fc6763',
  searchDesktop: '4801118d4a100f756e833d33984436a3899cff359c532f8fd3aaf174b60b3b49',
  addToPlaylist: '47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990',
  removeFromPlaylist: '47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990',
  moveItemsInPlaylist: '47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990',
};
const READ_OPERATIONS = new Set(['verifyVerificationPlaylist', 'status', 'search', 'listPlaylists', 'getPlaylist', 'getTrack', 'getArtist', 'getArtistAlbums', 'getArtistTracks', 'getAlbum', 'getAlbumTracks']);
const WRITE_OPERATIONS = new Set(['applySyncPreview', 'createVerificationPlaylist']);
const ERRORS = {
  INPUT: ['UNSUPPORTED', 'The Spotify worker request is invalid.'],
  AUTH: ['SIGN_IN_REQUIRED', 'Spotify authentication is unavailable. Sign in again and retry.'],
  VERIFY: ['VERIFICATION_REQUIRED', 'Spotify requires verification. Complete it in the browser and retry.'],
  RATE: ['RATE_LIMITED', 'Spotify is rate limiting requests. Try again shortly.'],
  API_DENIED: ['API_DENIED', 'Spotify denied catalog access for this session or application.'],
  NOT_FOUND: ['TRACK_NOT_FOUND', 'Spotify does not expose this catalog item through the current API.'],
  PROVIDER_CHANGED: ['PROVIDER_CHANGED', 'Spotify returned a response that did not match the requested catalog item.'],
  CURSOR_RESET_REQUIRED: ['CURSOR_RESET_REQUIRED', 'The Spotify profile cursor expired or changed. Refresh the profile and start again.'],
  DEADLINE: ['REQUEST_TIMEOUT', 'The Spotify request timed out. Try again shortly.'],
  PROVIDER: ['PROVIDER_CHANGED', 'Spotify returned an unsupported response.'],
  CREATION_OUTCOME_UNKNOWN: ['OUTCOME_UNKNOWN', 'Spotify playlist creation may have succeeded. Do not retry creation; inspect the provider library.'],
  OUTCOME_UNKNOWN: ['OUTCOME_UNKNOWN', 'Spotify may have received playlist changes. Refresh the playlist before trying again.'],
};

function fail(kind, diagnostics = null) {
  const [code, message] = ERRORS[kind] || ERRORS.PROVIDER;
  const error = { code, message };
  if (diagnostics && diagnostics.phase !== 'worker_start' && /^[a-z_]{1,40}$/.test(diagnostics.phase || '')) {
    error.phase = diagnostics.phase;
    if (kind === 'RATE' && diagnostics.phase === 'owner_profile') {
      error.message = 'Spotify temporarily limited profile browsing. Try again shortly.';
      if (Number.isInteger(diagnostics.retryAfterSeconds)) {
        error.message = `Spotify temporarily limited profile browsing. Try again in ${diagnostics.retryAfterSeconds} seconds.`;
      }
    } else {
      error.message += ` (phase=${diagnostics.phase}${Number.isInteger(diagnostics.httpStatus) ? `, HTTP ${diagnostics.httpStatus}` : ''})`;
      if (Number.isInteger(diagnostics.retryAfterSeconds)) error.message += ` (retry-after=${diagnostics.retryAfterSeconds}s)`;
    }
  }
  if (diagnostics?.phase === 'catalog_graphql' && CATALOG_GRAPHQL_OPERATIONS.has(diagnostics.operationName)) error.operationName = diagnostics.operationName;
  if (diagnostics && Number.isInteger(diagnostics.httpStatus)) error.httpStatus = diagnostics.httpStatus;
  if (diagnostics && Number.isInteger(diagnostics.retryAfterSeconds)) error.retryAfterSeconds = diagnostics.retryAfterSeconds;
  return { ok: false, error };
}
function validateCookieHeader(value) {
  if (typeof value !== 'string' || !value || value.length > 16_384 || /[\r\n\u0000-\u001f\u007f]/u.test(value)) throw new Error('invalid');
  const names = new Set();
  for (const part of value.split(';')) {
    const pair = part.trim().split('=');
    if (pair.length < 2 || !['sp_dc', 'sp_key'].includes(pair[0])) throw new Error('invalid');
    if (names.has(pair[0]) || !pair.slice(1).join('=').trim()) throw new Error('invalid');
    names.add(pair[0]);
  }
  if (!names.has('sp_dc')) throw new Error('invalid');
  return value.trim();
}

function allowedRequest(rawUrl, method) {
  let url;
  try { url = new URL(rawUrl); } catch { return false; }
  if (url.protocol !== 'https:' || url.username || url.password || url.hash) return false;
  if (url.hostname === 'open.spotify.com') {
    if (method === 'GET' && url.pathname === '/api/server-time' && url.search === '') return true;
    if (method === 'GET' && url.pathname === '/api/token') {
      const keys = [...url.searchParams.keys()].sort().join(',');
      return keys === 'productType,reason,totp,totpServer,totpVer' && url.searchParams.get('reason') === 'transport' && url.searchParams.get('productType') === 'web-player';
    }
    return false;
  }
  if (url.hostname === 'api.spotify.com') {
    if (method === 'POST') return url.pathname === '/v1/me/playlists' && url.search === '';
    if (method !== 'GET') return false;
    if (/^\/v1\/tracks\/[A-Za-z0-9]{22}$/.test(url.pathname)) return url.search === '';
    return /^\/v1\/(?:me|users|playlists|artists|albums)\//.test(url.pathname);
  }
  if (url.hostname === 'spclient.wg.spotify.com' && method === 'GET') {
    const paged = url.pathname.match(/^\/user-profile-view\/v3\/profile\/([A-Za-z0-9._~!$&'()*+,;=:@%-]+)\/playlists$/);
    if (paged) {
      let id; try { id = decodeURIComponent(paged[1]); } catch { return false; }
      if (!id || id.length > 180 || /[\s\u0000-\u001f\u007f/\\?#]/u.test(id) || encodeURIComponent(id) !== paged[1]) return false;
      const keys = [...url.searchParams.keys()].sort().join(',');
      return keys === 'limit,offset' && /^(?:[1-9]\d?|1\d{2}|200)$/.test(url.searchParams.get('limit') || '')
        && /^(?:0|[1-9]\d{0,7})$/.test(url.searchParams.get('offset') || '')
        && Number(url.searchParams.get('offset')) <= 10_000_000;
    }
    const match = url.pathname.match(/^\/user-profile-view\/v3\/profile\/([A-Za-z0-9._~!$&'()*+,;=:@%-]+)$/);
    if (!match) return false;
    let id;
    try { id = decodeURIComponent(match[1]); } catch { return false; }
    if (!id || id.length > 180 || /[\s\u0000-\u001f\u007f/\\?#]/u.test(id) || encodeURIComponent(id) !== match[1]) return false;
    const keys = [...url.searchParams.keys()].sort().join(',');
    return keys === 'artist_limit,episode_limit,market,playlist_limit'
      && /^(?:[1-9]\d?|1\d{2}|200)$/.test(url.searchParams.get('playlist_limit') || '')
      && url.searchParams.get('artist_limit') === '0'
      && url.searchParams.get('episode_limit') === '0'
      && url.searchParams.get('market') === 'from_token';
  }
  if (url.hostname === 'api-partner.spotify.com') return method === 'POST' && url.pathname === '/pathfinder/v2/query' && url.search === '';
  if (url.hostname === 'api.github.com') return method === 'GET' && url.href === REGISTRY_URL;
  if (url.hostname === 'francescograzioso.github.io') return method === 'GET' && url.href === HASH_REGISTRY_URL;
  return false;
}

function requestOperationName(url, options) {
  if (url.hostname !== 'api-partner.spotify.com') return null;
  try {
    const name = JSON.parse(options.body || '{}').operationName;
    return CATALOG_GRAPHQL_OPERATIONS.has(name) ? name : null;
  } catch { return null; }
}
function requestPhase(url, options) {
  if (url.hostname === 'open.spotify.com') return url.pathname === '/api/token' ? 'token_exchange' : 'spotify_clock';
  if (url.hostname === 'api.github.com') return 'totp_registry';
  if (url.hostname === 'francescograzioso.github.io') return 'hash_registry';
  if (url.hostname === 'api-partner.spotify.com') {
    try { return JSON.parse(options.body || '{}').operationName === 'profileAttributes' ? 'account_profile' : 'catalog_graphql'; } catch { return 'catalog_graphql'; }
  }
  if (url.hostname === 'spclient.wg.spotify.com') return 'owner_profile';
  if (url.hostname === 'api.spotify.com' && url.pathname === '/v1/me/playlists' && options.method === 'POST') return 'playlist_create';
  if (url.hostname === 'api.spotify.com') return /^\/v1\/users\//.test(url.pathname) ? 'owner_profile' : 'catalog_rest';
  return 'provider_request';
}

function makeFetcher(cookieHeader, deadlineAt, fetchImpl = globalThis.fetch, diagnostics = null, creationAccountId = null, onMutationDispatch = () => {}, creationIntentId = undefined) {
  const creationTitle=verificationTitle(creationIntentId);
  return async (input, options = {}, transportMetadata = {}) => {
    const url = new URL(typeof input === 'string' ? input : input.url);
    const method = String(options.method || 'GET').toUpperCase();
    if (!allowedRequest(url.href, method)) throw new Error('blocked');
    if (url.hostname === 'api.spotify.com' && method === 'POST') {
      if (!creationAccountId || !creationTitle || url.pathname !== '/v1/me/playlists'
          || options.body !== JSON.stringify({ name: creationTitle, public: false, collaborative: false })) throw new Error('blocked');
    }
    if (diagnostics) { diagnostics.phase = requestPhase(url, options); diagnostics.operationName = requestOperationName(url, options); diagnostics.httpStatus = null; diagnostics.retryAfterSeconds = null; }
    if (Date.now() >= deadlineAt) throw new Error('deadline');
    options.signal?.throwIfAborted();
    const remaining = Math.max(1, Math.min(8_000, deadlineAt - Date.now()));
    const controller = new AbortController();
    const onCallerAbort = () => controller.abort();
    const headers = new Headers(options.headers || {});
    headers.delete('cookie');
    headers.delete('authorization');
    if (url.hostname === 'open.spotify.com') headers.set('Cookie', cookieHeader);
    if (url.hostname === 'api.spotify.com' || url.hostname === 'api-partner.spotify.com' || url.hostname === 'spclient.wg.spotify.com') {
      const authorization = new Headers(options.headers || {}).get('authorization');
      if (!authorization || !/^Bearer [A-Za-z0-9._~-]{8,4096}$/.test(authorization)) throw new Error('blocked');
      headers.set('Authorization', authorization);
    }
    const body = options.body;
    if (body !== undefined && (typeof body !== 'string' || body.length > 64 * 1024)) throw new Error('blocked');
    const timer = setTimeout(() => controller.abort(), remaining);
    options.signal?.addEventListener('abort', onCallerAbort, { once: true });
    if (options.signal?.aborted) controller.abort();
    try {
      const requestOptions = { method, headers, ...(body === undefined ? {} : { body }), redirect: 'error', signal: controller.signal, cache: 'no-store' };
      // All local gates and request setup must complete before recording dispatch.
      if (Date.now() >= deadlineAt) throw new Error('deadline');
      controller.signal.throwIfAborted();
      if (url.hostname === 'api.spotify.com' && method === 'POST') onMutationDispatch();
      const response = await fetchImpl(url, requestOptions);
      if (diagnostics) {
        diagnostics.httpStatus = response.status;
        const retryAfter = response.headers.get('retry-after');
        if (response.status === 429 && retryAfter && /^\d{1,4}$/.test(retryAfter)) diagnostics.retryAfterSeconds = Math.min(3600, Number(retryAfter));
      }
      const binaryProfilePage = url.hostname === 'spclient.wg.spotify.com' && (/\/playlists$/.test(url.pathname) || (transportMetadata.profileDiagnostic === true && /^\/user-profile-view\/v3\/profile\/[A-Za-z0-9._~!$&'()*+,;=:@%-]+$/.test(url.pathname)));
      if (binaryProfilePage && transportMetadata.profileDiagnostic === true && [401, 403, 429].includes(response.status)) return response;
      if (binaryProfilePage) {
        const reader = response.body?.getReader();
        let bytes;
        if (reader) {
          const chunks = []; let size = 0;
          while (true) { const { done, value } = await reader.read(); if (done) break; size += value.byteLength; if (size > MAX_RESPONSE) { await reader.cancel(); if (transportMetadata.profileDiagnostic === true) { const headers = new Headers(response.headers); headers.set('x-burrata-profile-oversize', '1'); return new Response(new Uint8Array(), { status: response.status, statusText: response.statusText, headers }); } throw new Error('oversize'); } chunks.push(value); }
          bytes = new Uint8Array(size); let cursor = 0; for (const chunk of chunks) { bytes.set(chunk, cursor); cursor += chunk.byteLength; }
        } else { bytes = new Uint8Array(await response.arrayBuffer()); if (bytes.byteLength > MAX_RESPONSE) { if (transportMetadata.profileDiagnostic === true) { const headers = new Headers(response.headers); headers.set('x-burrata-profile-oversize', '1'); return new Response(new Uint8Array(), { status: response.status, statusText: response.statusText, headers }); } throw new Error('oversize'); } }
        return new Response(bytes, { status: response.status, statusText: response.statusText, headers: response.headers });
      }
      const reader = response.body?.getReader();
      let text;
      if (reader) {
        const chunks = [];
        let bytes = 0;
        const decoder = new TextDecoder();
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          bytes += value.byteLength;
          if (bytes > MAX_RESPONSE) { await reader.cancel(); throw new Error('oversize'); }
          chunks.push(decoder.decode(value, { stream: true }));
        }
        chunks.push(decoder.decode());
        text = chunks.join('');
      } else text = await response.text();
      if (Buffer.byteLength(text, 'utf8') > MAX_RESPONSE) throw new Error('oversize');
      return new Response(text, { status: response.status, statusText: response.statusText, headers: response.headers });
    } finally { clearTimeout(timer); options.signal?.removeEventListener('abort', onCallerAbort); }
  };
}

function providerResult(value, accountId) {
  if (!value || value.__burrataProviderError || value.accountId !== accountId) throw new Error('provider');
  return value;
}

async function spotifySnapshot(provider, hashes, playlistId, accountId) {
  const entries = [], cursors = new Set();
  let cursor = null, playlist = null, total = null;
  for (let page = 0; page < 100; page += 1) {
    const result = providerResult(await provider.run('spotify', 'getPlaylist', { playlistId, limit: 100, ...(cursor ? { cursor } : {}) }, hashes), accountId);
    if (!Array.isArray(result.entries) || !result.playlist) throw new Error('provider');
    if (!playlist) { playlist = result.playlist; total = Number.isSafeInteger(result.total) ? result.total : null; }
    else if (result.playlist.id !== playlist.id || result.total !== total) throw new Error('provider');
    entries.push(...result.entries);
    if (entries.length > 10_000) throw new Error('provider');
    cursor = result.nextCursor || null;
    if (!cursor) break;
    if (cursors.has(cursor)) throw new Error('provider');
    cursors.add(cursor);
    if (page === 99) throw new Error('provider');
  }
  if (!playlist || playlist.id !== playlistId || playlist.canEdit !== true || playlist.ownerId !== accountId
      || total === null || entries.length !== total || entries.some(row => !row.available || !/^[A-Za-z0-9]{22}$/.test(row.trackId || '') || typeof row.entryId !== 'string' || !row.entryId)
      || new Set(entries.map(row => row.entryId)).size !== entries.length) throw new Error('provider');
  return { playlist, entries };
}

async function applyReviewedSpotifyPlan(provider, hashes, args, accountId, markDispatched) {
  const allowed = ['previewApproved', 'playlistId', 'expectedEntries', 'removeEntryIds', 'addTrackUris', 'desiredTrackUris', 'syncPreview'];
  const preview = args.syncPreview;
  if (Object.keys(args).some(key => !allowed.includes(key)) || args.previewApproved !== true
      || !/^[A-Za-z0-9]{22}$/.test(args.playlistId || '') || !preview || Object.keys(preview).length !== 5
      || typeof preview.previewId !== 'string' || !/^[A-Za-z0-9-]{8,128}$/.test(preview.previewId)
      || preview.accountId !== accountId || typeof preview.baselineRevision !== 'string' || preview.baselineRevision.length > 256
      || typeof preview.remoteRevision !== 'string' || preview.remoteRevision.length > 256
      || typeof preview.remoteHash !== 'string' || !/^[a-f0-9]{64}$/i.test(preview.remoteHash)) throw new Error('provider');
  const before = await spotifySnapshot(provider, hashes, args.playlistId, accountId);
  const expected = args.expectedEntries;
  if (!Array.isArray(expected) || expected.length !== before.entries.length || expected.length > 10_000
      || expected.some((row, i) => !row || Object.keys(row).length !== 2 || row.entryId !== before.entries[i].entryId || row.trackId !== before.entries[i].trackId)) throw new Error('conflict');
  const removed = args.removeEntryIds, added = args.addTrackUris, desired = args.desiredTrackUris;
  if (!Array.isArray(removed) || !Array.isArray(added) || !Array.isArray(desired) || removed.length + added.length > 500
      || removed.some(id => typeof id !== 'string' || !before.entries.some(entry => entry.entryId === id))
      || new Set(removed).size !== removed.length
      || added.some(uri => !/^spotify:track:[A-Za-z0-9]{22}$/.test(uri))
      || desired.length > 10_000 || desired.some(uri => !/^spotify:track:[A-Za-z0-9]{22}$/.test(uri))) throw new Error('provider');
  const remaining = new Map(), target = new Map();
  for (const entry of before.entries) if (!removed.includes(entry.entryId)) remaining.set(entry.trackId, (remaining.get(entry.trackId) || 0) + 1);
  for (const uri of added) { const id = uri.slice('spotify:track:'.length); remaining.set(id, (remaining.get(id) || 0) + 1); }
  for (const uri of desired) { const id = uri.slice('spotify:track:'.length); target.set(id, (target.get(id) || 0) + 1); }
  if (JSON.stringify([...remaining].sort()) !== JSON.stringify([...target].sort())) throw new Error('conflict');
  let applied = 0;
  const mutate = async (operation, mutationArgs) => {
    const count = operation === 'moveTracks' ? 1 : mutationArgs.entryIds?.length || mutationArgs.trackUris?.length || 0;
    if (applied + count > 500) throw new Error('limit');
    markDispatched();
    const receipt = providerResult(await provider.run('spotify', operation, mutationArgs, hashes), accountId);
    if (!receipt.receipt || receipt.receipt.playlistId !== args.playlistId) throw new Error('provider');
    applied += count;
  };
  for (let i = 0; i < removed.length; i += 100) await mutate('removeTracks', { playlistId: args.playlistId, entryIds: removed.slice(i, i + 100) });
  for (let i = 0; i < added.length; i += 100) await mutate('addTracks', { playlistId: args.playlistId, trackUris: added.slice(i, i + 100) });
  const after = await spotifySnapshot(provider, hashes, args.playlistId, accountId);
  const queues = new Map();
  for (const entry of after.entries) { if (!queues.has(entry.trackId)) queues.set(entry.trackId, []); queues.get(entry.trackId).push(entry.entryId); }
  const desiredIds = desired.map(uri => queues.get(uri.slice('spotify:track:'.length))?.shift());
  if (desiredIds.some(id => !id) || [...queues.values()].some(ids => ids.length)) throw new Error('provider');
  const order = after.entries.map(entry => entry.entryId);
  for (let i = desiredIds.length - 1; i >= 0; i -= 1) {
    const id = desiredIds[i], position = order.indexOf(id), next = i + 1 < desiredIds.length ? desiredIds[i + 1] : null;
    if (position < 0) throw new Error('provider');
    if ((next === null && position !== order.length - 1) || (next !== null && position + 1 !== order.indexOf(next))) {
      order.splice(position, 1);
      const destination = next === null ? order.length : order.indexOf(next);
      if (destination < 0) throw new Error('provider');
      order.splice(destination, 0, id);
      await mutate('moveTracks', { playlistId: args.playlistId, entryIds: [id], beforeEntryId: next });
    }
  }
  const verified = await spotifySnapshot(provider, hashes, args.playlistId, accountId);
  if (verified.entries.map(entry => `spotify:track:${entry.trackId}`).join('\n') !== desired.join('\n')) throw new Error('provider');
  return { accountId, playlistId: args.playlistId, applied, status: 'VERIFIED' };
}

// Creation receipts never contain provider bodies, account IDs, URLs or credentials.
// No receipt on a transport failure means UNKNOWN, never an inferred false dispatch.
export async function executeJob(job, options = {}) {
  if (job?.operation !== 'createVerificationPlaylist') return executeJobInner(job, options);
  const trace = { mutationDispatched: false, phase: 'worker_start' };
  const requestId = job.creationRequestId;
  if (typeof requestId !== 'string' || !/^[a-f0-9]{32}$/.test(requestId)) return { ok: false, error: { code: 'UNSUPPORTED', message: 'Creation requires a native attempt ID.' } };
  let response;
  try { response = await executeJobInner(job, { ...options, creationTrace: trace }); }
  catch { response = { ok: false, error: { code: 'OUTCOME_UNKNOWN', message: 'Creation outcome is unknown. Do not retry creation.' } }; trace.mutationDispatched = null; }
  const phase = /^[a-z_]{1,40}$/.test(response?.error?.phase || '') ? response.error.phase : trace.phase;
  const id = response?.result?.playlistId;
  const validId = /^[A-Za-z0-9]{22}$/.test(typeof id === 'string' ? id : '');
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

async function executeJobInner(job, { fetchImpl = globalThis.fetch, deadlineMs = DEADLINE_MS, creationTrace = null } = {}) {
  const diagnostics = { phase: 'worker_start', httpStatus: null };
  if (creationTrace) Object.defineProperty(diagnostics, 'phase', { get: () => creationTrace.phase, set: value => { creationTrace.phase = value; }, enumerable: true });
  let cookieHeader, mutationDispatched = false;
  const authRefresh = job?.internalAuthRefresh === true;
  if (!job || job.provider !== 'spotify' || !(authRefresh || READ_OPERATIONS.has(job.operation) || WRITE_OPERATIONS.has(job.operation)) || !job.args || typeof job.args !== 'object' || Array.isArray(job.args)) return fail('INPUT', diagnostics);
  try { cookieHeader = validateCookieHeader(job.cookieHeader); } catch { return fail('INPUT', diagnostics); }
  const deadlineAt = Date.now() + Math.max(1, Math.min(DEADLINE_MS, Number(deadlineMs) || DEADLINE_MS));
  try {
    const fetcher = makeFetcher(cookieHeader, deadlineAt, fetchImpl, diagnostics, job.operation === 'createVerificationPlaylist' && typeof job.args.accountId === 'string' ? job.args.accountId : null, () => { mutationDispatched = true; if (creationTrace) creationTrace.mutationDispatched = true; }, job.args?.verificationIntentId);
    const [authSource, providerSource] = await Promise.all([
      readFile(resolve(ROOT, 'extension/spotify-auth.js'), 'utf8'),
      readFile(resolve(ROOT, 'extension/provider-operations.js'), 'utf8'),
    ]);
    const context = vm.createContext({
      fetch: fetcher, URL, URLSearchParams, Response, Headers, AbortController, TextEncoder,
      crypto: webcrypto, TextDecoder, setTimeout, clearTimeout, Date, Math, Number, String, JSON, Array, Object, RegExp, Map, Set,
      location: { hostname: 'open.spotify.com', origin: 'https://open.spotify.com' },
    });
    context.globalThis = context;
    vm.runInContext(authSource, context, { timeout: 1000, filename: 'spotify-auth.js' });
    vm.runInContext(providerSource, context, { timeout: 1000, filename: 'provider-operations.js' });
    const storage = { get: async () => ({}), set: async () => {} };
    let code = null, nuance = null;
    const cached = job.cachedAuth && typeof job.cachedAuth.accessToken === 'string' && Number.isSafeInteger(job.cachedAuth.expiresAt) && job.cachedAuth.expiresAt > Date.now() + 30000 ? job.cachedAuth : null;
    if (authRefresh || !cached) {
      nuance = await context.BurrataSpotifyAuth.loadRegistry(storage, fetcher);
      const timeResponse = await fetcher('https://open.spotify.com/api/server-time', { headers: { Accept: 'application/json' } });
      if (!timeResponse.ok) return fail(timeResponse.status === 429 ? 'RATE' : 'AUTH', diagnostics);
      const timeBody = await timeResponse.json();
      code = await context.BurrataSpotifyAuth.totp(nuance.secret, Number(timeBody.serverTime));
    }
    if (authRefresh) {
      const query = new URLSearchParams({ reason: 'transport', productType: 'web-player', totp: code, totpServer: code, totpVer: String(nuance.version) });
      const response = await fetcher(`https://open.spotify.com/api/token?${query}`, { headers: { Accept: 'application/json' } });
      if (!response.ok) return fail(response.status === 429 ? 'RATE' : 'AUTH', diagnostics);
      const body = await response.json();
      const reportedExpiry = Number.isSafeInteger(body.accessTokenExpirationTimestampMs) ? body.accessTokenExpirationTimestampMs : (Number.isFinite(body.expiresIn) && body.expiresIn > 0 && body.expiresIn <= 86400 ? Date.now() + Math.floor(body.expiresIn * 1000) : null);
      if (body.isAnonymous === true || typeof body.accessToken !== 'string' || !body.accessToken) return fail('AUTH', diagnostics);
      const cacheable = Number.isSafeInteger(reportedExpiry) && reportedExpiry > Date.now() + 30000 && reportedExpiry <= Date.now() + 86400000;
      const expiresAt = cacheable ? reportedExpiry : Date.now() + 60000;
      return { ok: true, internalAuth: { accessToken: body.accessToken, expiresAt, cacheable } };
    }
    const hashes = Object.fromEntries(Object.entries(HASHES).map(([name, hash]) => [name, { hash }]));
    try {
      const response = await fetcher(HASH_REGISTRY_URL, { headers: { Accept: 'application/json' } });
      if (response.ok) {
        const registry = await response.json();
        for (const name of Object.keys(HASHES)) {
          const item = registry.operations?.[name];
          const hash = typeof item?.hash === 'string' && /^[a-f0-9]{64}$/i.test(item.hash) ? item.hash : null;
          const previous = typeof item?.previous_hash === 'string' && /^[a-f0-9]{64}$/i.test(item.previous_hash) ? item.previous_hash : null;
          if (hash) hashes[name] = ['addToPlaylist', 'removeFromPlaylist', 'moveItemsInPlaylist'].includes(name)
            ? { hash }
            : { hash, previous_hash: previous, fallback: HASHES[name] };
        }
      }
    } catch { /* Built-in hashes remain available if the data-only registry is unavailable. */ }
    if (cached) hashes.cachedAccessToken = cached.accessToken;
    else hashes.totp = { code, version: nuance.version };
    let value;
    if (job.operation === 'applySyncPreview') {
      const verifiedAccount = providerResult(await context.BurrataSpotifyProvider.run('spotify', 'status', {}, hashes, { fetchImpl: fetcher }), job.args.syncPreview?.accountId);
      value = await applyReviewedSpotifyPlan(context.BurrataSpotifyProvider, hashes, job.args, verifiedAccount.accountId, () => { mutationDispatched = true; if (creationTrace) creationTrace.mutationDispatched = true; });
    } else {
      value = await context.BurrataSpotifyProvider.run('spotify', job.operation, job.args, hashes, { fetchImpl: fetcher });
    }
    if (value?.__burrataProviderError) {
      if (mutationDispatched) return fail(job.operation === 'createVerificationPlaylist' ? 'CREATION_OUTCOME_UNKNOWN' : 'OUTCOME_UNKNOWN', diagnostics);
      const code = value.__burrataProviderError.code;
      if (code === 'SIGN_IN_REQUIRED') return fail('AUTH', diagnostics);
      if (code === 'VERIFICATION_REQUIRED') return fail('VERIFY', diagnostics);
      if (code === 'RATE_LIMITED') return fail('RATE', diagnostics);
      if (code === 'REQUEST_TIMEOUT') return fail('DEADLINE', diagnostics);
      if (code === 'API_DENIED') return fail('API_DENIED', diagnostics);
      if (['TRACK_NOT_FOUND', 'ARTIST_NOT_FOUND', 'ALBUM_NOT_FOUND'].includes(code)) return fail('NOT_FOUND', diagnostics);
      if (code === 'PROVIDER_CHANGED') return fail('PROVIDER_CHANGED', diagnostics);
      if (code === 'CURSOR_RESET_REQUIRED') return fail('CURSOR_RESET_REQUIRED', diagnostics);
      return fail('PROVIDER', diagnostics);
    }
    const serialized = JSON.stringify(value);
    if (!serialized || serialized.length > MAX_OUTPUT || /(?:Bearer\s+[A-Za-z0-9._~-]{8,}|accessToken|cookieHeader|sp_dc=|sp_key=|"(?:secret|authorization|cookie)"\s*:)/i.test(serialized)) return fail(mutationDispatched ? (job.operation === 'createVerificationPlaylist' ? 'CREATION_OUTCOME_UNKNOWN' : 'OUTCOME_UNKNOWN') : 'PROVIDER', diagnostics);
    return { ok: true, result: value };
  } catch (error) {
    if (mutationDispatched) return fail(job.operation === 'createVerificationPlaylist' ? 'CREATION_OUTCOME_UNKNOWN' : 'OUTCOME_UNKNOWN', diagnostics);
    if (Date.now() >= deadlineAt || error?.name === 'AbortError' || String(error?.message) === 'deadline') return fail('DEADLINE', diagnostics);
    if (String(error?.message) === 'blocked') return fail('INPUT', diagnostics);
    return fail('PROVIDER', diagnostics);
  }
}

async function main() {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > MAX_INPUT) { process.stdout.write(JSON.stringify(fail('INPUT')) + '\n'); return; }
    chunks.push(chunk);
  }
  let response;
  try { response = await executeJob(JSON.parse(Buffer.concat(chunks).toString('utf8'))); } catch { response = fail('INPUT'); }
  const output = JSON.stringify(response);
  process.stdout.write((output.length <= MAX_OUTPUT ? output : JSON.stringify(fail('PROVIDER'))) + '\n');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => { process.stdout.write(JSON.stringify(fail('PROVIDER')) + '\n'); });
}

export { allowedRequest, makeFetcher };
