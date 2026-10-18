import test from 'node:test';
import assert from 'node:assert/strict';
import { copyFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { webcrypto } from 'node:crypto';
import { allowedRequest, executeJob, makeFetcher } from './provider-session-worker.mjs';

test('cancelled Spotify requests never dispatch provider work', async () => {
  const controller = new AbortController();
  controller.abort();
  let dispatched = false;
  const fetch = makeFetcher('sp_dc=fixture', Date.now() + 10000, async () => {
    dispatched = true;
    return Response.json({});
  });
  await assert.rejects(fetch('https://open.spotify.com/api/server-time', {signal:controller.signal}), {name:'AbortError'});
  assert.equal(dispatched, false);
});

test('Spotify cancellation reaches an in-flight provider request', async () => {
  const controller = new AbortController();
  const fetch = makeFetcher('sp_dc=fixture', Date.now() + 10000, async (_url, options) => {
    return new Promise((_resolve, reject) => {
      options.signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), {once:true});
      controller.abort();
    });
  });
  await assert.rejects(fetch('https://open.spotify.com/api/server-time', {signal:controller.signal}), {name:'AbortError'});
});

const SECRET_COOKIE = 'sp_dc=fixture-cookie-secret; sp_key=fixture-key-secret';
test('paged profile allowlist accepts only bounded canonical offset and limit queries', () => {
  const base = 'https://spclient.wg.spotify.com/user-profile-view/v3/profile/owner123/playlists';
  for (const [offset, limit] of [[0, 1], [30, 30], [999, 200], [1000, 50], [10002, 30], [10000000, 50]]) assert.equal(allowedRequest(`${base}?offset=${offset}&limit=${limit}`, 'GET'), true);
  for (const query of ['offset=00&limit=30', 'offset=-1&limit=30', 'offset=10000001&limit=30', 'offset=100000000&limit=30', 'offset=0&limit=201', 'offset=0&limit=30&market=US', 'limit=30']) assert.equal(allowedRequest(`${base}?${query}`, 'GET'), false);
  assert.equal(allowedRequest(`${base.replace('owner123', 'owner%2Fother')}?offset=0&limit=30`, 'GET'), false);
  const summaryBase = 'https://spclient.wg.spotify.com/user-profile-view/v3/profile/owner123';
  assert.equal(allowedRequest(`${summaryBase}?playlist_limit=200&artist_limit=0&episode_limit=0&market=from_token`, 'GET'), true);
  for (const query of [
    'playlist_limit=201&artist_limit=0&episode_limit=0&market=from_token',
    'playlist_limit=200&artist_limit=1&episode_limit=0&market=from_token',
    'playlist_limit=200&artist_limit=0&episode_limit=0&market=US',
    'playlist_limit=200&artist_limit=0&episode_limit=0&market=from_token&extra=1',
  ]) assert.equal(allowedRequest(`${summaryBase}?${query}`, 'GET'), false);
  assert.equal(allowedRequest(`${summaryBase.replace('owner123','owner%2Fother')}?playlist_limit=200&artist_limit=0&episode_limit=0&market=from_token`, 'GET'), false);
});

function protoVarint(value) {
  let n = BigInt(value), out = [];
  while (n > 127n) { out.push(Number(n & 127n) | 128); n >>= 7n; }
  out.push(Number(n)); return out;
}
function protoField(number, wire, payload) { return [...protoVarint((number << 3) | wire), ...payload]; }
function protoString(number, value) { const bytes = [...new TextEncoder().encode(value)]; return protoField(number, 2, [...protoVarint(bytes.length), ...bytes]); }
function protoInt(number, value) { return protoField(number, 0, protoVarint(value)); }
function protoPlaylist(index, overrideId = null, ownerUri = 'spotify:user:owner123') {
  const id = overrideId || String(index + 1).padStart(22, '0');
  const item = [
    ...protoString(1, `spotify:playlist:${id}`), ...protoString(2, `Fixture ${index + 1}`),
    ...protoString(3, `https://i.scdn.co/image/fixture-${index + 1}`), ...protoInt(4, index * 10),
    ...protoString(5, 'Fixture Owner'), ...protoString(6, ownerUri), ...protoInt(7, 1),
    // Unknown fields with length-delimited and deprecated group wire types must be skipped safely.
    ...protoString(20, 'future-field'), ...protoField(21, 3, [...protoInt(22, 7), ...protoField(21, 4, [])]),
  ];
  return item;
}
function protoProfilePage(offset, limit, total = 5, idOverrides = null, ownerOverrides = null) {
  const rows = Array.from({ length: Math.min(limit, Math.max(0, total - offset)) }, (_, i) => protoPlaylist(offset + i, idOverrides?.[offset + i], ownerOverrides?.[offset + i] || 'spotify:user:owner123'));
  return new Uint8Array([...rows.flatMap(item => protoField(1, 2, [...protoVarint(item.length), ...item])), ...protoInt(2, total)]);
}
function protoProfileSlice(offset, count, total, idOverrides = {}) {
  const rows = Array.from({ length: count }, (_, i) => protoPlaylist(offset + i, idOverrides[i] || null));
  return new Uint8Array([...rows.flatMap(item => protoField(1, 2, [...protoVarint(item.length), ...item])), ...protoInt(2, total)]);
}

test('paged Spotify owner profile reads protobuf pages, verifies adjacent boundaries, and preserves playlist occurrences', async () => {
  const calls = [];
  const fetchImpl = async (url, options = {}) => {
    const target = new URL(url).href; calls.push({ target, options });
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') {
      const request = JSON.parse(options.body);
      if (request.operationName === 'profileAttributes') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account', name: 'Fixture Account' } } } });
    }
    const parsed = new URL(target);
    if (parsed.hostname === 'spclient.wg.spotify.com' && parsed.pathname.endsWith('/playlists')) {
      const offset = Number(parsed.searchParams.get('offset')), limit = Number(parsed.searchParams.get('limit'));
      return new Response(protoProfilePage(offset, limit), { headers: { 'content-type': 'application/x-protobuf' } });
    }
    throw new Error('unexpected fixture URL');
  };
  const job = cursor => executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2, ...(cursor ? { cursor } : {}) }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl });
  const first = await job(null); assert.equal(first.ok, true, JSON.stringify(first));
  assert.equal(first.result.items.length, 2); assert.equal(first.result.totalCount, 5); assert.equal(first.result.complete, false);
  assert.equal(first.result.snapshotConsistency, 'progressive');
  assert.equal(first.result.items[0].title, 'Fixture 1'); assert.equal(first.result.items[0].artworkUrl, 'https://i.scdn.co/image/fixture-1');
  const cursor1 = first.result.nextCursor; assert.match(cursor1, /^sp-profile-v5:2:5:[a-f0-9]{64}:[a-f0-9]{64}:\d+:0:2:2:[a-f0-9]{64}:5:0$/);
  let otherAccountProfileRequests = 0;
  const otherAccount = await executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2, cursor: cursor1 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:another-account' } } } });
    if (new URL(target).hostname === 'spclient.wg.spotify.com') otherAccountProfileRequests += 1;
    throw new Error('unexpected fixture URL');
  } });
  assert.equal(otherAccount.ok, false); assert.equal(otherAccountProfileRequests, 0);
  const second = await job(cursor1); assert.equal(second.ok, true, JSON.stringify(second));
  assert.equal(JSON.stringify(second.result.items.map(item => item.title)), JSON.stringify(['Fixture 3', 'Fixture 4']));
  assert.match(second.result.nextCursor, /^sp-profile-v5:4:5:[a-f0-9]{64}:[a-f0-9]{64}:\d+:2:2:2:[a-f0-9]{64}:5:0$/);
  const third = await job(second.result.nextCursor); assert.equal(third.ok, true, JSON.stringify(third));
  assert.equal(JSON.stringify(third.result.items.map(item => item.title)), JSON.stringify(['Fixture 5']));
  assert.equal(third.result.nextCursor, null); assert.equal(third.result.complete, true); assert.equal(third.result.truncated, false);
  assert.equal(calls.filter(call => call.target.includes('/playlists')).length, 5);
  const profileRequests = calls.filter(call => call.target.includes('/playlists'));
  assert.ok(profileRequests.every(({ options }) => options.headers.get('accept') === 'application/x-protobuf'));
  assert.ok(profileRequests.every(({ options }) => options.redirect === 'error'));
  assert.ok(profileRequests.every(({ options }) => /^Bearer /.test(options.headers.get('authorization')) && !options.headers.has('cookie')));
  assert.equal(JSON.stringify({ first, second, third }).includes('TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture'), false);
  assert.equal(JSON.stringify({ first, second, third }).includes(SECRET_COOKIE), false);
});
test('empty public profile accepts protobuf default-zero total and rows without a reported total remain visible as incomplete', async () => {
  const input = { provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } };
  const run = body => executeJob(input, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    return new Response(body, { headers: { 'content-type': 'application/x-protobuf' } });
  } });
  const empty = await run(new Uint8Array());
  assert.equal(empty.ok, true, JSON.stringify(empty)); assert.equal(empty.result.items.length, 0);
  assert.equal(empty.result.totalCount, 0); assert.equal(empty.result.nextCursor, null);
  assert.equal(empty.result.truncated, false); assert.equal(empty.result.complete, true);
  const playlistWithoutTotal = protoPlaylist(0);
  const pageWithoutTotal = new Uint8Array([...protoField(1, 2, [...protoVarint(playlistWithoutTotal.length), ...playlistWithoutTotal])]);
  const partial = await run(pageWithoutTotal); assert.equal(partial.ok, true, JSON.stringify(partial));
  assert.equal(partial.result.items.length, 1); assert.equal(partial.result.totalCount, null); assert.ok(partial.result.nextCursor);
  assert.equal(partial.result.truncated, true); assert.equal(partial.result.complete, false);
});

test('opt-in Spotify profile response inspection returns a finite shape DTO without response strings', async () => {
  const input = { provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 30, inspectProfileResponse: true }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } };
  let lastProfileUrl = null;
  const run = async (body, contentType = 'application/x-protobuf', status = 200, inspect = true, offset = 0, limit = 30, cursor = undefined) => executeJob({ ...input, args: { ...input.args, inspectProfileResponse: inspect, offset, limit, ...(cursor === undefined ? {} : { cursor }) } }, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    if (new URL(target).hostname === 'spclient.wg.spotify.com') lastProfileUrl = target;
    return new Response(body, { status, headers: contentType ? { 'content-type': contentType } : {} });
  } });
  const valid = await run(protoProfilePage(0, 30, 2));
  assert.equal(valid.ok, true, JSON.stringify(valid));
  assert.equal(JSON.stringify(valid.result.items), '[]');
  const shape = valid.result.profileResponseShape;
  assert.deepEqual(Object.keys(shape), ['version', 'responseMode', 'httpStatus', 'contentTypeClass', 'bodyBytes', 'parseClass', 'failureClass', 'decodedRows', 'decodedTotal', 'playlistIdHashes', 'topLevelFields']);
  assert.equal(shape.version, 1); assert.equal(shape.httpStatus, 200); assert.equal(shape.contentTypeClass, 'protobuf');
  assert.equal(shape.parseClass, 'valid'); assert.equal(shape.failureClass, 'none'); assert.equal(shape.decodedRows, 2); assert.equal(shape.decodedTotal, 2);
  assert.equal(JSON.stringify(shape.topLevelFields), JSON.stringify([{ field: 1, wire: 2, count: 2 }, { field: 2, wire: 0, count: 1 }]));
  assert.ok(shape.bodyBytes > 0 && shape.bodyBytes <= 2 * 1024 * 1024);
  assert.doesNotMatch(JSON.stringify(shape), /owner123|fixture-account|TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture|fixture-cookie-secret|spotify:|https?:/i);

  const knownPage = protoProfilePage(0, 30, 2);
  for (const contentType of ['application/octet-stream', 'application/vnd.spotify.profile+protobuf', '']) {
    const compatible = await run(knownPage, contentType, 200, false);
    assert.equal(compatible.ok, true, `${contentType || 'missing MIME'}: ${JSON.stringify(compatible)}`);
    assert.equal(compatible.result.items.length, 2);
    const inspected = await run(knownPage, contentType, 200, true);
    assert.equal(inspected.ok, true);
    assert.equal(inspected.result.profileResponseShape.contentTypeClass, contentType.includes('protobuf') ? 'protobuf' : contentType ? 'other' : 'missing');
    assert.equal(inspected.result.profileResponseShape.parseClass, 'valid');
    assert.equal(inspected.result.profileResponseShape.failureClass, 'none');
  }
  for (const contentType of ['application/x-protobuf', 'application/octet-stream']) {
    const emptyBinary = await run(new Uint8Array(), contentType, 200, false);
    assert.equal(emptyBinary.ok, true, `${contentType}: ${JSON.stringify(emptyBinary)}`);
    assert.equal(emptyBinary.result.items.length, 0); assert.equal(emptyBinary.result.totalCount, 0);
  }
  const emptyUnknown = await run(new Uint8Array(), 'application/x-unknown-binary', 200, true);
  assert.equal(emptyUnknown.ok, true); assert.equal(emptyUnknown.result.profileResponseShape.failureClass, 'content_type');
  const emptyUnknownNormal = await run(new Uint8Array(), 'application/x-unknown-binary', 200, false);
  assert.equal(emptyUnknownNormal.ok, false); assert.equal(emptyUnknownNormal.error.code, 'PROVIDER_CHANGED');
  const html200 = await run('<html>fixture</html>', 'text/html; charset=utf-8', 200, true);
  assert.equal(html200.result.profileResponseShape.contentTypeClass, 'text'); assert.equal(html200.result.profileResponseShape.parseClass, 'not_binary');
  assert.equal(html200.result.profileResponseShape.failureClass, 'content_type');
  const htmlNormal = await run('<html>fixture</html>', 'text/html', 200, false);
  assert.equal(htmlNormal.ok, false); assert.equal(htmlNormal.error.code, 'PROVIDER_CHANGED');
  const unknownFieldsOnly = new Uint8Array([...protoString(20, 'future-only'), ...protoInt(21, 1)]);
  const unknownShape = await run(unknownFieldsOnly, 'application/x-protobuf', 200, true);
  assert.equal(unknownShape.result.profileResponseShape.parseClass, 'malformed'); assert.equal(unknownShape.result.profileResponseShape.failureClass, 'response_shape');
  const unknownNormal = await run(unknownFieldsOnly, 'application/x-protobuf', 200, false);
  assert.equal(unknownNormal.ok, false); assert.equal(unknownNormal.error.code, 'PROVIDER_CHANGED');
  const brokenWire = await run(new Uint8Array([0x0a, 0xff]), 'application/octet-stream', 200, true);
  assert.equal(brokenWire.result.profileResponseShape.parseClass, 'malformed'); assert.equal(brokenWire.result.profileResponseShape.failureClass, 'response_shape');
  const emptyMissing = await run(new Uint8Array(), '', 200, true);
  assert.equal(emptyMissing.result.profileResponseShape.failureClass, 'content_type');
  const emptyJson = await run(new Uint8Array(), 'application/json', 200, false);
  assert.equal(emptyJson.ok, false); assert.equal(emptyJson.error.code, 'PROVIDER_CHANGED');


  const continuationSlice = await run(protoProfilePage(60, 2, 110), 'application/x-protobuf', 200, true, 60, 2);
  assert.equal(continuationSlice.ok, true); assert.equal(continuationSlice.result.profileResponseShape.failureClass, 'none');
  assert.equal(continuationSlice.result.profileResponseShape.decodedRows, 2); assert.equal(continuationSlice.result.profileResponseShape.decodedTotal, 110);
  assert.equal(new URL(lastProfileUrl).searchParams.get('offset'), '60'); assert.equal(new URL(lastProfileUrl).searchParams.get('limit'), '2');
  lastProfileUrl = null;
  const cursorRejected = await run(protoProfilePage(0, 30, 2), 'application/x-protobuf', 200, true, 0, 30, 'sp-profile-v3:1:110:bad:bad:1:0:0000000000000000000001');
  assert.equal(cursorRejected.ok, false); assert.equal(lastProfileUrl, null);
  const finalSlice = await run(protoProfilePage(108, 2, 110), 'application/octet-stream', 200, true, 108, 2);
  assert.equal(finalSlice.result.profileResponseShape.failureClass, 'none'); assert.equal(finalSlice.result.profileResponseShape.decodedRows, 2);
  const beyondEnd = await run(protoProfilePage(110, 1, 110), 'application/octet-stream', 200, true, 110, 1);
  assert.equal(beyondEnd.result.profileResponseShape.failureClass, 'none'); assert.equal(beyondEnd.result.profileResponseShape.decodedRows, 0);
  const json200 = await run(JSON.stringify({ owner: 'sensitive owner string', accessToken: 'sensitive token' }), 'application/json; charset=utf-8');
  assert.equal(json200.ok, true); assert.equal(json200.result.profileResponseShape.contentTypeClass, 'json');
  assert.equal(json200.result.profileResponseShape.parseClass, 'not_binary'); assert.equal(json200.result.profileResponseShape.failureClass, 'content_type');
  assert.doesNotMatch(JSON.stringify(json200.result.profileResponseShape), /sensitive|owner|token/i);

  const malformed = await run(new Uint8Array([0]));
  assert.equal(malformed.ok, true); assert.equal(malformed.result.profileResponseShape.parseClass, 'malformed');
  assert.equal(malformed.result.profileResponseShape.failureClass, 'response_shape');

  const wrongCount = await run(protoProfilePage(0, 1, 5));
  assert.equal(wrongCount.result.profileResponseShape.parseClass, 'valid'); assert.equal(wrongCount.result.profileResponseShape.failureClass, 'page_count');
  const duplicate = await run(protoProfilePage(0, 2, 2, ['0000000000000000000001', '0000000000000000000001']));
  assert.equal(duplicate.result.profileResponseShape.failureClass, 'duplicate_id');
  const invalidId = await run(protoProfilePage(0, 1, 1, ['bad-id']));
  assert.equal(invalidId.result.profileResponseShape.failureClass, 'invalid_id');
  const mixedOwners = await run(protoProfilePage(0, 30, 2, null, ['spotify:user:owner123', 'spotify:user:another-public-owner']));
  assert.equal(mixedOwners.result.profileResponseShape.failureClass, 'none');

  const oversize = await run(new Uint8Array(2 * 1024 * 1024 + 1));
  assert.equal(oversize.ok, true); assert.equal(oversize.result.profileResponseShape.parseClass, 'oversize');
  assert.equal(oversize.result.profileResponseShape.bodyBytes, 2 * 1024 * 1024);

  for (const status of [401, 403]) { const denied = await run(new Uint8Array(), 'application/x-protobuf', status); assert.equal(denied.ok, false); assert.equal(denied.error.code, 'SIGN_IN_REQUIRED'); assert.equal(denied.error.phase, 'owner_profile'); }
  const limited = await run(new Uint8Array(), 'application/x-protobuf', 429); assert.equal(limited.ok, false); assert.equal(limited.error.code, 'RATE_LIMITED');
  const defaultOff = await run(JSON.stringify({ error: 'fixture' }), 'application/json', 200, false);
  assert.equal(defaultOff.ok, false); assert.equal(defaultOff.error.code, 'PROVIDER_CHANGED'); assert.equal(defaultOff.result, undefined);
});

test('ownerless Spotify profile cursors never dispatch GraphQL or profile requests', async () => {
  const calls = [];
  for (const ownerId of [undefined, null, '']) {
    for (const cursor of ['sp-profile-v5:30:110:fixture', 'sp-profile-v4:legacy', 'sp-profile-future:opaque']) {
      const result = await executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId, cursor }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async (url) => {
        calls.push(new URL(url).hostname);
        return Response.json({ operations: {} });
      } });
      assert.equal(result.ok, false);
      assert.equal(result.error.code, 'CURSOR_RESET_REQUIRED');
      assert.match(result.error.message, /Refresh the profile/);
    }
  }
  assert.equal(calls.includes('api-partner.spotify.com'), false);
  assert.equal(calls.includes('spclient.wg.spotify.com'), false);
});

test('legacy profile cursors require refresh instead of an expensive prefix migration', async () => {
  const calls = [];
  const run = cursor => executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', cursor, limit: 2 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href; calls.push(target);
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    throw new Error('profile request should not run for legacy cursor');
  } });
  for (const cursor of ['sp-profile-v4:30:110:' + 'a'.repeat(64) + ':' + 'b'.repeat(64) + ':1:0:30:30:' + 'c'.repeat(64) + ':110:0', 'sp-profile-v1:2:' + 'a'.repeat(64) + ':' + 'b'.repeat(64), 'sp-profile-v2:2:5:' + 'a'.repeat(64) + ':' + 'b'.repeat(64) + ':' + 'c'.repeat(64), 'sp-profile-v3:2:5:' + 'a'.repeat(64) + ':' + 'b'.repeat(64) + ':1:0:0000000000000000000001,0000000000000000000002']) {
    const result = await run(cursor); assert.equal(result.ok, false); assert.equal(result.error.code, 'CURSOR_RESET_REQUIRED'); assert.match(result.error.message, /Refresh the profile/);
  }
  assert.equal(calls.some(url => url.includes('/playlists')), false);
});

test('v5 profile cursor reaches offsets above 10000 in at most two verified page requests', async () => {
  const digest = async value => Array.from(new Uint8Array(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), b => b.toString(16).padStart(2, '0')).join('');
  const ownerHash = await digest('owner123'), accountHash = await digest('fixture-account'), expires = Date.now() + 60_000;
  const id = index => String(index + 1).padStart(22, '0');
  const prevIds = `${id(10000)},${id(10001)}`;
  const prevHash = Array.from(new Uint8Array(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(prevIds))), b => b.toString(16).padStart(2, '0')).join('');
  const cursor = `sp-profile-v5:10002:12005:${ownerHash}:${accountHash}:${expires}:10000:2:2:${prevHash}:12005:0`;
  const calls = [];
  const result = await executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 30, cursor }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async (url, options = {}) => {
    const target = new URL(url).href; calls.push(target);
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') return new Response(protoProfilePage(Number(parsed.searchParams.get('offset')), Number(parsed.searchParams.get('limit')), 12005), { headers: { 'content-type': 'application/x-protobuf' } });
    throw new Error('unexpected fixture URL');
  } });
  assert.equal(result.ok, true, JSON.stringify(result)); assert.equal(result.result.items.length, 30);
  assert.equal(result.result.complete, false); assert.equal(result.result.snapshotConsistency, 'progressive');
  const profileCalls = calls.filter(url => url.includes('/playlists'));
  assert.equal(profileCalls.length, 2); assert.ok(profileCalls.some(url => url.includes('offset=10000&limit=2'))); assert.ok(profileCalls.some(url => url.includes('offset=10002&limit=30')));
  assert.match(result.result.nextCursor, /^sp-profile-v5:10032:12005:[a-f0-9]{64}:[a-f0-9]{64}:\d+:10002:30:30:[a-f0-9]{64}:12005:0$/);
});

test('v5 profile paging marks the worker safe-offset ceiling as truncated without blocking earlier manual pages', async () => {
  const digest = async value => Array.from(new Uint8Array(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), b => b.toString(16).padStart(2, '0')).join('');
  const ownerHash = await digest('owner123'), accountHash = await digest('fixture-account'), expires = Date.now() + 60_000;
  const anchorIds = [9999989, 9999990].map(value => String(value).padStart(22, '0'));
  const anchorHash = Array.from(new Uint8Array(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(anchorIds.join(',')))), b => b.toString(16).padStart(2, '0')).join('');
  const cursor = `sp-profile-v5:9999990:2147483647:${ownerHash}:${accountHash}:${expires}:9999988:2:2:${anchorHash}:2147483647:0`;
  const calls = [];
  const result = await executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 30, cursor }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href; calls.push(target);
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') return new Response(protoProfilePage(Number(parsed.searchParams.get('offset')), Number(parsed.searchParams.get('limit')), 2147483647), { headers: { 'content-type': 'application/x-protobuf' } });
    throw new Error('unexpected fixture URL');
  } });
  assert.equal(result.ok, true, JSON.stringify(result)); assert.equal(result.result.items.length, 30);
  assert.equal(result.result.nextCursor, null); assert.equal(result.result.truncated, true); assert.equal(result.result.complete, false);
  assert.equal(calls.filter(url => url.includes('/playlists')).length, 2);
});

test('v5 profile cursor binds owner, account, count state, previous page hash, and expiry', async () => {
  const digest = async value => Array.from(new Uint8Array(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), b => b.toString(16).padStart(2, '0')).join('');
  const ownerHash = await digest('owner123'), accountHash = await digest('fixture-account'), anchor = ['0000000000000000000001', '0000000000000000000002'];
  const calls = [];
  const anchorHash = await digest(anchor.join(','));
  const makeCursor = (owner = ownerHash, account = accountHash, total = 5, expires = Date.now() + 60_000) => `sp-profile-v5:2:${total}:${owner}:${account}:${expires}:0:2:2:${anchorHash}:${total}:0`;
  const run = (ownerId, cursor, mutate = false) => executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId, cursor, limit: 2 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href; calls.push(target);
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') {
      const offset = Number(parsed.searchParams.get('offset')), limit = Number(parsed.searchParams.get('limit'));
      if (mutate && offset === 0) return new Response(protoProfilePage(0, limit, 5, { 0: '9999999999999999999999' }), { headers: { 'content-type': 'application/x-protobuf' } });
      return new Response(protoProfilePage(offset, limit), { headers: { 'content-type': 'application/x-protobuf' } });
    }
    throw new Error('unexpected fixture URL');
  } });
  for (const [owner, value] of [['other-owner', makeCursor()], ['owner123', makeCursor(ownerHash, '0'.repeat(64))], ['owner123', makeCursor(ownerHash, accountHash, 5, Date.now() - 1)]]) {
    calls.length = 0; const denied = await run(owner, value); assert.equal(denied.ok, false); assert.equal(denied.error.code, 'CURSOR_RESET_REQUIRED'); assert.equal(calls.some(url => url.includes('/playlists')), false);
  }
  calls.length=0;const changedTotal=await run('owner123',makeCursor(ownerHash,accountHash,6));assert.equal(changedTotal.ok,true,JSON.stringify(changedTotal));assert.equal(changedTotal.result.totalCount,6);assert.equal(changedTotal.result.truncated,true);assert.equal(changedTotal.result.complete,false);assert.equal(calls.filter(url=>url.includes('/playlists')).length,2);
  calls.length = 0; const changedBoundary = await run('owner123', makeCursor(), true); assert.equal(changedBoundary.ok, false); assert.equal(changedBoundary.error.code, 'PROVIDER_CHANGED');
  assert.equal(calls.filter(url => url.includes('/playlists')).length, 1);
});
test('v5 profile continuation anchors refetch the full prior page at its original size', async () => {
  const digest = async value => Array.from(new Uint8Array(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), b => b.toString(16).padStart(2, '0')).join('');
  const ownerHash = await digest('owner123'), accountHash = await digest('fixture-account'), calls = [];
  const run = cursor => executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 30, ...(cursor ? { cursor } : {}) }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href; calls.push(target);
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') {
      const offset = Number(parsed.searchParams.get('offset')), limit = Number(parsed.searchParams.get('limit'));
      // Preserve the total anomaly while v4 verifies the full prior page at its original size.
      const total = offset === 28 && limit === 2 ? 100 : 110;
      return new Response(protoProfilePage(offset, limit, total), { headers: { 'content-type': 'application/x-protobuf' } });
    }
    throw new Error('unexpected fixture URL');
  } });
  const first = await run(null); assert.equal(first.ok, true, JSON.stringify(first));
  assert.equal(first.result.items.length, 30); assert.ok(first.result.nextCursor);
  calls.length = 0;
  const next = await run(first.result.nextCursor); assert.equal(next.ok, true, JSON.stringify(next));
  assert.equal(next.result.items.length, 30); assert.equal(next.result.totalCount, 110);
  const profileCalls = calls.filter(url => url.includes('/playlists')).map(url => new URL(url));
  assert.deepEqual(profileCalls.map(url => [url.searchParams.get('offset'), url.searchParams.get('limit')]), [['0', '30'], ['30', '30']]);
  assert.match(first.result.nextCursor, new RegExp(`^sp-profile-v5:30:110:${ownerHash}:${accountHash}:\\d+:0:30:30:[a-f0-9]{64}:110:0$`));
});
test('v5 profile cursor preserves a caller page size of 50 for continuation anchors', async () => {
  const calls = [];
  const run = cursor => executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 50, ...(cursor ? { cursor } : {}) }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href; calls.push(target);
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') {
      const offset = Number(parsed.searchParams.get('offset'));
      return new Response(protoProfilePage(offset, Number(parsed.searchParams.get('limit')), 70), { headers: { 'content-type': 'application/x-protobuf' } });
    }
    throw new Error('unexpected fixture URL');
  } });
  const first = await run(null); assert.equal(first.ok, true, JSON.stringify(first)); assert.equal(first.result.items.length, 50);
  calls.length = 0;
  const second = await run(first.result.nextCursor); assert.equal(second.ok, true, JSON.stringify(second)); assert.equal(second.result.items.length, 20);
  const profileCalls = calls.filter(url => url.includes('/playlists')).map(url => new URL(url));
  assert.deepEqual(profileCalls.map(url => [url.searchParams.get('offset'), url.searchParams.get('limit')]), [['0', '50'], ['50', '50']]);
});
test('v5 profile paging preserves sparse rows and honest incompleteness across changing totals', async () => {
  const pages = new Map([[0, [30, 110]], [30, [30, 110]], [60, [29, 109]], [90, [10, 90]], [120, [0, 90]]]);
  const runCalls = [];
  const run = cursor => executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 30, ...(cursor ? { cursor } : {}) }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href; runCalls.push(target);
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') {
      const offset = Number(parsed.searchParams.get('offset')), limit = Number(parsed.searchParams.get('limit'));
      const [count, total] = pages.get(offset) || [0, 90];
      return new Response(protoProfileSlice(offset, Math.min(count, limit), total), { headers: { 'content-type': 'application/x-protobuf' } });
    }
    throw new Error('unexpected fixture URL');
  } });
  let cursor = null, result;
  for (const [index, expectedCount] of [30, 30, 29, 10, 0].entries()) {
    runCalls.length = 0; result = await run(cursor); assert.equal(result.ok, true, `page ${index}: ${JSON.stringify(result)}`);
    assert.equal(result.result.items.length, expectedCount); assert.equal(result.result.totalCount, 110);
    if (expectedCount === 0) { assert.equal(result.result.nextCursor, null); assert.equal(result.result.truncated, true); assert.equal(result.result.complete, false); }
    else { assert.ok(result.result.nextCursor); cursor = result.result.nextCursor; }
    if (index > 0) assert.equal(runCalls.filter(url => url.includes('/playlists')).length, 2);
  }
  assert.equal(result.result.snapshotConsistency, 'progressive');
});
test('v5 profile paging returns cross-page overlaps for UI dedupe and flags the count as incomplete', async () => {
  const run = cursor => executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2, ...(cursor ? { cursor } : {}) }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') {
      const offset = Number(parsed.searchParams.get('offset'));
      const body = offset === 2 ? protoProfileSlice(2, 2, 4, { 0: '0000000000000000000002' }) : protoProfileSlice(offset, Math.min(2, Math.max(0, 4 - offset)), 4);
      return new Response(body, { headers: { 'content-type': 'application/x-protobuf' } });
    }
    throw new Error('unexpected fixture URL');
  } });
  const first = await run(null); assert.equal(first.ok, true); assert.equal(first.result.items.length, 2);
  const next = await run(first.result.nextCursor); assert.equal(next.ok, true, JSON.stringify(next));
  assert.equal(next.result.items.length, 2); assert.equal(next.result.items[0].id, first.result.items[1].id);
  assert.equal(next.result.truncated, true); assert.equal(next.result.complete, false); assert.ok(next.result.nextCursor);
});
test('v4 known-empty terminal pages cannot claim completion below the cursor total', async () => {
  const digest = async value => Array.from(new Uint8Array(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), b => b.toString(16).padStart(2, '0')).join('');
  const ownerHash = await digest('owner123'), accountHash = await digest('fixture-account'), anchor = ['0000000000000000000001', '0000000000000000000002'];
  const cursor = `sp-profile-v5:2:5:${ownerHash}:${accountHash}:${Date.now() + 60_000}:0:2:2:${await digest(anchor.join(','))}:5:0`;
  const run = executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2, cursor }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } }, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const parsed = new URL(target); if (parsed.hostname === 'spclient.wg.spotify.com') {
      const offset = Number(parsed.searchParams.get('offset'));
      return new Response(offset === 0 ? protoProfileSlice(0, 2, 0) : new Uint8Array(), { headers: { 'content-type': 'application/x-protobuf' } });
    }
    throw new Error('unexpected fixture URL');
  } });
  const result = await run; assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.items.length, 0); assert.equal(result.result.totalCount, 5);
  assert.equal(result.result.nextCursor, null); assert.equal(result.result.truncated, true); assert.equal(result.result.complete, false);
});
test('unknown-MIME zero-byte terminal is incomplete only after a verified profile continuation', async () => {
  const input = { provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } };
  const invoke = (args, profileResponse) => executeJob({ ...input, args: { ...input.args, ...args } }, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    const offset = Number(new URL(target).searchParams.get('offset'));
    const chosen = await profileResponse(offset);
    return new Response(chosen.body, { status: chosen.status ?? 200, headers: chosen.contentType ? { 'content-type': chosen.contentType } : {} });
  } });
  const initialUnknown = await invoke({}, async () => ({ body: new Uint8Array(), contentType: 'application/x-unknown-binary' }));
  assert.equal(initialUnknown.ok, false); assert.equal(initialUnknown.error.code, 'PROVIDER_CHANGED');
  const firstPage = await invoke({}, async offset => ({ body: protoProfilePage(offset, 2, 6), contentType: 'application/x-protobuf' }));
  assert.equal(firstPage.ok, true); assert.ok(firstPage.result.nextCursor);
  const badAnchor = await invoke({ cursor: firstPage.result.nextCursor }, async () => ({ body: new Uint8Array(), contentType: 'application/x-unknown-binary' }));
  assert.equal(badAnchor.ok, false); assert.equal(badAnchor.error.code, 'PROVIDER_CHANGED');

  let cursor = firstPage.result.nextCursor;
  const secondPage = await invoke({ cursor }, async offset => ({ body: protoProfilePage(offset, 2, 6), contentType: 'application/x-protobuf' }));
  assert.equal(secondPage.ok, true); assert.equal(secondPage.result.items.length, 2); cursor = secondPage.result.nextCursor;
  const terminal = await invoke({ cursor }, async offset => offset === 4
    ? { body: new Uint8Array(), contentType: 'application/x-unknown-binary' }
    : { body: protoProfilePage(offset, 2, 6), contentType: 'application/x-protobuf' });
  assert.equal(terminal.ok, true, JSON.stringify(terminal)); assert.equal(terminal.result.items.length, 0);
  assert.equal(terminal.result.totalCount, 6); assert.equal(terminal.result.nextCursor, null);
  assert.equal(terminal.result.truncated, true); assert.equal(terminal.result.complete, false);

  for (const contentType of ['text/html', 'application/json']) {
    const rejected = await invoke({ cursor }, async offset => offset === 4
      ? { body: new Uint8Array(), contentType }
      : { body: protoProfilePage(offset, 2, 6), contentType: 'application/x-protobuf' });
    assert.equal(rejected.ok, false, `${contentType}: ${JSON.stringify(rejected)}`); assert.equal(rejected.error.code, 'PROVIDER_CHANGED');
  }
  const noContent = await invoke({ cursor }, async offset => offset === 4
    ? { body: new Uint8Array(), contentType: 'application/x-unknown-binary', status: 204 }
    : { body: protoProfilePage(offset, 2, 6), contentType: 'application/x-protobuf' });
  assert.equal(noContent.ok, false); assert.equal(noContent.error.code, 'PROVIDER_CHANGED');
});
test('bounded protobuf parser rejects invalid UTF-8, overlong varints, and oversized streams', async () => {
  const input = { provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } };
  const invalidUtf8 = new Uint8Array([0x0a, 0x03, 0x12, 0x01, 0xff, 0x10, 0x01]);
  const overlongVarint = new Uint8Array([0x10, ...Array(10).fill(0x80)]);
  const run = body => executeJob(input, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    return new Response(body, { headers: { 'content-type': 'application/x-protobuf' } });
  } });
  for (const malformed of [invalidUtf8, overlongVarint]) { const result = await run(malformed); assert.equal(result.ok, false); assert.equal(result.error.phase, 'owner_profile'); assert.equal(result.error.httpStatus, 200); }
  const oversized = await run(new Uint8Array(2 * 1024 * 1024 + 1)); assert.equal(oversized.ok, false); assert.equal(oversized.error.phase, 'owner_profile'); assert.equal(oversized.error.httpStatus, 200);
});
test('protobuf group skipping validates inner and matching end field numbers', async () => {
  const input = { provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } };
  const run = body => executeJob(input, { fetchImpl: async url => {
    const target = new URL(url).href;
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
    return new Response(body, { headers: { 'content-type': 'application/x-protobuf' } });
  } });
  // A valid unknown group with a valid nested unknown field remains skippable.
  assert.equal((await run(protoProfilePage(0, 1, 1))).ok, true);
  const malformedPlaylist = payload => {
    const row = [...protoPlaylist(0), ...protoField(21, 3, payload)];
    return new Uint8Array([...protoField(1, 2, [...protoVarint(row.length), ...row]), ...protoInt(2, 1)]);
  };
  const oversizedFieldTag = protoVarint(536870912n * 8n);
  const cases = [
    malformedPlaylist([0, ...protoField(21, 4, [])]),
    malformedPlaylist([...oversizedFieldTag, ...protoField(21, 4, [])]),
    malformedPlaylist([...protoField(22, 0, protoVarint(1)), ...protoField(22, 4, [])]),
  ];
  for (const bytes of cases) {
    const result = await run(bytes); assert.equal(result.ok, false); assert.equal(result.error.phase, 'owner_profile');
  }
});

test('binary owner-profile HTTP errors keep phase, status, and rate-limit retry-after diagnostics', async () => {
  const input = { provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'owner123', limit: 2 }, cookieHeader: SECRET_COOKIE, cachedAuth: { accessToken: 'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture', expiresAt: Date.now() + 60000 } };
  for (const [status, headers, body, code] of [
    [401, { 'content-type': 'application/json' }, '{"error":"expired"}', 'SIGN_IN_REQUIRED'],
    [403, { 'content-type': 'text/plain' }, 'denied', 'SIGN_IN_REQUIRED'],
    [429, { 'content-type': 'application/json', 'retry-after': '17' }, '{"error":"slow down"}', 'RATE_LIMITED'],
  ]) {
    const result = await executeJob(input, { fetchImpl: async url => {
      const target = new URL(url).href;
      if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
      if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account' } } } });
      return new Response(body, { status, headers });
    } });
    assert.equal(result.ok, false); assert.equal(result.error.code, code); assert.equal(result.error.phase, 'owner_profile');
    assert.equal(result.error.httpStatus, status);
    if (status === 429) { assert.equal(result.error.retryAfterSeconds, 17); assert.match(result.error.message, /17 seconds/); }
  }
});
function fixtureFetch(calls) {
  return async (url, options = {}) => {
    const target = new URL(url).href;
    calls.push({ target, options });
    if (target === 'https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef') {
      return Response.json({ files: { 'registry.json': { content: JSON.stringify([{ s: 'JBSWY3DPEHPK3PXP', v: 61 }]) } } });
    }
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://open.spotify.com/api/server-time') return Response.json({ serverTime: 100000 });
    if (new URL(target).pathname === '/api/token') return Response.json({ accessToken: 'TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture', isAnonymous: false });
    if (new URL(target).pathname === '/v1/tracks/1234567890123456789012') return Response.json({ id: '1234567890123456789012', name: 'Exact fixture track', duration_ms: 245480, artists: [{ name: 'Fixture Artist' }], album: { name: 'Fixture Album' } });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') {
      const request = JSON.parse(options.body);
      if (request.operationName === 'profileAttributes') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account', name: 'Fixture Account' } } } });
      return Response.json({ data: { searchV2: { tracksV2: { totalCount: 1, items: [{ item: { data: { id: '12345678901234567890', name: 'Fixture Song', artists: [{ name: 'Fixture Artist' }], albumOfTrack: { name: 'Fixture Album' }, duration: { totalMilliseconds: 181250 } } } }] } } } });
    }
    throw new Error('unexpected fixture URL');
  };
}

test('Spotify exact-track route allowlist accepts only the fixed GET path', () => {
  assert.equal(allowedRequest('https://api.spotify.com/v1/tracks/1234567890123456789012', 'GET'), true);
  assert.equal(allowedRequest('https://api.spotify.com/v1/tracks/1234567890123456789012?market=US', 'GET'), false);
  assert.equal(allowedRequest('https://api.spotify.com/v1/tracks/not-an-id', 'GET'), false);
  assert.equal(allowedRequest('https://api.spotify.com/v1/tracks/1234567890123456789012/related-artists', 'GET'), false);
  assert.equal(allowedRequest('https://api.spotify.com/v1/tracks/1234567890123456789012', 'POST'), false);
});

test('prototype is opt-in, rejects raw mutations, and never emits cookies or access tokens', async () => {
  const calls = [];
  assert.deepEqual(await executeJob({ provider: 'spotify', operation: 'addTracks', args: {}, cookieHeader: SECRET_COOKIE }, { fetchImpl: fixtureFetch(calls) }), {
    ok: false, error: { code: 'UNSUPPORTED', message: 'The Spotify worker request is invalid.' },
  });
  assert.equal(calls.length, 0);

  const result = await executeJob({ provider: 'spotify', operation: 'search', args: { query: 'Fixture', kind: 'track', limit: 30 }, cookieHeader: SECRET_COOKIE }, { fetchImpl: fixtureFetch(calls) });
  assert.equal(result.ok, true);
  assert.equal(result.result.accountId, 'fixture-account');
  assert.equal(result.result.items[0].title, 'Fixture Song');
  const output = JSON.stringify(result);
  assert.equal(output.includes(SECRET_COOKIE), false);
  assert.equal(output.includes('fixture-cookie-secret'), false);
  assert.equal(output.includes('TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture'), false);
  assert.ok(calls.every(({ target }) => ['open.spotify.com', 'api.spotify.com', 'api-partner.spotify.com', 'spclient.wg.spotify.com', 'api.github.com', 'francescograzioso.github.io'].includes(new URL(target).hostname)));
  for (const { target, options } of calls) {
    const host = new URL(target).hostname;
    if (host !== 'open.spotify.com') {
      assert.equal(new Headers(options.headers).has('cookie'), false);
      if (!['api.spotify.com', 'api-partner.spotify.com', 'spclient.wg.spotify.com'].includes(host)) assert.equal(new Headers(options.headers).has('authorization'), false);
    }
    if (host === 'open.spotify.com') assert.equal(new Headers(options.headers).get('cookie'), SECRET_COOKIE);
    assert.equal(options.redirect, 'error');
  }
});

test('native Spotify worker preserves permanent catalog denial and per-track not-found codes', async () => {
  for (const [status, code] of [[403, 'API_DENIED'], [404, 'TRACK_NOT_FOUND']]) {
    const calls = [];
    const base = fixtureFetch(calls);
    const fetchImpl = async (url, options) => {
      if (new URL(url).pathname === '/v1/tracks/1234567890123456789012') {
        calls.push({ target: new URL(url).href, options });
        return new Response('{"error":{"message":"fixture"}}', { status });
      }
      return base(url, options);
    };
    const result = await executeJob({ provider: 'spotify', operation: 'getTrack', args: { trackId: '1234567890123456789012' }, cookieHeader: SECRET_COOKIE }, { fetchImpl });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, code);
    assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
  }
});

test('native Spotify worker routes exact track lookup through the authenticated read operation', async () => {
  const calls = [];
  const result = await executeJob({ provider: 'spotify', operation: 'getTrack', args: { trackId: '1234567890123456789012' }, cookieHeader: SECRET_COOKIE }, { fetchImpl: fixtureFetch(calls) });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.accountId, 'fixture-account');
  assert.equal(result.result.items[0].id, '1234567890123456789012');
  assert.equal(result.result.items[0].data.durationSeconds, 245.48);
  const lookup = calls.find(call => new URL(call.target).pathname === '/v1/tracks/1234567890123456789012');
  assert.ok(lookup);
  assert.equal(new Headers(lookup.options.headers).get('authorization'), 'Bearer TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture');
  assert.equal(new Headers(lookup.options.headers).has('cookie'), false);
  assert.equal(lookup.options.redirect, 'error');
  assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
  assert.equal(JSON.stringify(result).includes('TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture'), false);
});

test('native Spotify worker routes artist lookup through the authenticated Pathfinder read operation', async () => {
  const artistId = 'A'.repeat(22), calls = [], base = fixtureFetch(calls);
  const fetchImpl = async (url, options = {}) => {
    if (new URL(url).pathname === '/pathfinder/v2/query' && JSON.parse(options.body).operationName === 'queryArtistMinimal') {
      calls.push({ target: new URL(url).href, options });
      return Response.json({ data: { artistUnion: { __typename: 'Artist', uri: `spotify:artist:${artistId}`, profile: { name: 'Fixture Artist' }, visuals: { avatarImage: { sources: [{ url: 'https://i.scdn.co/image/artist' }] } }, stats: { followers: 0 } } } });
    }
    return base(url, options);
  };
  const result = await executeJob({ provider: 'spotify', operation: 'getArtist', args: { artistId }, cookieHeader: SECRET_COOKIE }, { fetchImpl });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.accountId, 'fixture-account');
  assert.equal(result.result.artist.id, artistId);
  assert.equal(result.result.artist.title, 'Fixture Artist');
  assert.equal(result.result.artist.artworkUrl, 'https://i.scdn.co/image/artist');
  assert.equal(result.result.artist.data.followers, 0);
  const lookup = calls.find(call => new URL(call.target).pathname === '/pathfinder/v2/query' && JSON.parse(call.options.body).operationName === 'queryArtistMinimal');
  assert.ok(lookup);
  assert.equal(new Headers(lookup.options.headers).get('authorization'), 'Bearer TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture');
  assert.equal(new Headers(lookup.options.headers).has('cookie'), false);
  assert.equal(JSON.parse(lookup.options.body).extensions.persistedQuery.sha256Hash, '53d3f76582c49ad0a05dc685955f20dc2a5f2209b192e5446e5e4e623ce23a48');
  assert.deepEqual(JSON.parse(lookup.options.body).variables, { uri: `spotify:artist:${artistId}` });
  assert.equal(lookup.options.redirect, 'error');
  assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
  assert.equal(JSON.stringify(result).includes('TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture'), false);
});

test('native Spotify worker preserves artist ListPlatform fractional duration and missing-nanos default', async () => {
  const artistId = 'A'.repeat(22), trackA = 'B'.repeat(22), trackB = 'C'.repeat(22), calls = [], base = fixtureFetch(calls);
  const artistRow = (id, name, duration) => ({ uid: `uid-${id}`, entity: { __typename: 'EntityResponseWrapper', _uri: `spotify:track:${id}`, data: {
    __typename: 'Entity', uri: `spotify:track:${id}`, entityTypeTrait: { __typename: 'EntityTypeTrait', type: 'ENTITY_TYPE_TRACK' },
    identityTrait: { __typename: 'IdentityTrait', name, contributors: { items: [{ name: 'Fixture Artist', uri: `spotify:artist:${artistId}` }] } },
    consumptionExperienceTrait: { __typename: 'ConsumptionExperienceTrait', duration },
  } } });
  const rows = [artistRow(trackA, 'Fractional', { seconds: 131, nanoSeconds: 706000000 }), artistRow(trackB, 'Whole second', { seconds: 131 })];
  const fetchImpl = async (url, options = {}) => {
    if (new URL(url).pathname === '/pathfinder/v2/query') {
      const request = JSON.parse(options.body);
      if (request.operationName === 'profileAttributes') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account', name: 'Fixture Account' } } } });
      if (request.operationName === 'getListsContents') {
        calls.push({ target: new URL(url).href, options });
        return Response.json({ data: { lists: [{ __typename: 'List', items: { totalCount: 2, items: rows } }] } });
      }
    }
    return base(url, options);
  };
  const result = await executeJob({ provider: 'spotify', operation: 'getArtistTracks', args: { artistId }, cookieHeader: SECRET_COOKIE }, { fetchImpl });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(result.result.items.map(item => item.durationSeconds), [131.706, 131]);
  assert.deepEqual(result.result.items.map(item => item.trackId), [trackA, trackB]);
  assert.equal(result.result.items[0].position, 1); assert.equal(result.result.items[1].position, 2);
  const lookup = calls.find(call => new URL(call.target).pathname === '/pathfinder/v2/query' && JSON.parse(call.options.body).operationName === 'getListsContents');
  assert.ok(lookup);
  const request = JSON.parse(lookup.options.body);
  assert.deepEqual(request.variables, { uris: [`spotify:list:popular-release-segments-main-roles:artist_${artistId}`], offset: 0, limit: 30 });
  assert.equal(request.extensions.persistedQuery.sha256Hash, '130589ef0ac5ffc096b08ab11c031292812f91620665af1a339112ca3016c5c1');
  assert.equal(new Headers(lookup.options.headers).has('cookie'), false);
  assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
  assert.equal(JSON.stringify(result).includes('TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture'), false);
});

test('native Spotify worker preserves explicit artist NotFound and rejects mismatched identities', async () => {
  const artistId = 'A'.repeat(22), calls = [], base = fixtureFetch(calls);
  let responseMode = 'not-found';
  const fetchImpl = async (url, options) => {
    if (new URL(url).pathname === '/pathfinder/v2/query' && JSON.parse(options.body).operationName === 'queryArtistMinimal') {
      calls.push({ target: new URL(url).href, options });
      const artistUnion = responseMode === 'not-found' ? { __typename: 'NotFound' } : { __typename: 'Artist', uri: `spotify:artist:${'B'.repeat(22)}`, profile: { name: 'Wrong artist' } };
      return Response.json({ data: { artistUnion } });
    }
    return base(url, options);
  };
  const result = await executeJob({ provider: 'spotify', operation: 'getArtist', args: { artistId }, cookieHeader: SECRET_COOKIE }, { fetchImpl });
  assert.equal(result.ok, false);assert.equal(result.error.code, 'TRACK_NOT_FOUND');
  responseMode = 'mismatch';
  const mismatch = await executeJob({ provider: 'spotify', operation: 'getArtist', args: { artistId }, cookieHeader: SECRET_COOKIE }, { fetchImpl });
  assert.equal(mismatch.ok, false);assert.equal(mismatch.error.code, 'PROVIDER_CHANGED');
  assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
});

test('native Spotify worker pages artist releases with verified GraphQL offset variables', async () => {
  const artistId = 'A'.repeat(22), albumA = 'B'.repeat(22), albumB = 'C'.repeat(22), calls = [], base = fixtureFetch(calls);
  const fetchImpl = async (url, options = {}) => {
    if (new URL(url).pathname === '/pathfinder/v2/query' && JSON.parse(options.body).operationName === 'queryArtistDiscographyAll') {
      calls.push({ target: new URL(url).href, options });
      const request = JSON.parse(options.body), offset = request.variables.offset, albumId = offset === 0 ? albumA : albumB;
      return Response.json({ data: { artistUnion: { __typename: 'Artist', uri: `spotify:artist:${artistId}`, discography: { all: { totalCount: 2, items: [{ releases: { items: [{ __typename: 'Album', uri: `spotify:album:${albumId}`, name: `Release ${offset}`, type: 'ALBUM', date: { year: 2024 }, artists: { items: [{ profile: { name: 'Fixture Artist' } }] }, coverArt: { sources: [{ url: `https://i.scdn.co/image/${offset}` }] }, tracks: { totalCount: 0 } }] } }] } } } } });
    }
    return base(url, options);
  };
  const job = cursor => executeJob({ provider: 'spotify', operation: 'getArtistAlbums', args: { artistId, limit: 30, ...(cursor ? { cursor } : {}) }, cookieHeader: SECRET_COOKIE }, { fetchImpl });
  const first = await job(null);assert.equal(first.ok, true, JSON.stringify(first));assert.equal(first.result.items[0].id, albumA);assert.equal(first.result.total, 2);assert.equal(first.result.nextCursor, '1');
  const second = await job(first.result.nextCursor);assert.equal(second.ok, true, JSON.stringify(second));assert.equal(second.result.items[0].id, albumB);assert.equal(second.result.nextCursor, null);
  const pages = calls.filter(call => new URL(call.target).pathname === '/pathfinder/v2/query' && JSON.parse(call.options.body).operationName === 'queryArtistDiscographyAll').map(call => JSON.parse(call.options.body));
  assert.deepEqual(pages.map(request => [request.variables.limit, request.variables.offset, request.variables.order]), [[20, 0, 'DATE_DESC'], [20, 1, 'DATE_DESC']]);
  assert.ok(pages.every(request => request.extensions.persistedQuery.sha256Hash === '5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599'));
  for (const lookup of calls.filter(call => new URL(call.target).pathname === '/pathfinder/v2/query' && JSON.parse(call.options.body).operationName === 'queryArtistDiscographyAll')) {
    assert.equal(new Headers(lookup.options.headers).get('authorization'), 'Bearer TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture');assert.equal(new Headers(lookup.options.headers).has('cookie'), false);assert.equal(lookup.options.redirect, 'error');
  }
  assert.equal(JSON.stringify(first).includes(SECRET_COOKIE), false);assert.equal(JSON.stringify(second).includes('TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture'), false);
});

test('native Spotify worker pages rich album tracks through getAlbum and marks nonzero offsets experimental', async () => {
  const albumId = 'D'.repeat(22), trackA = 'E'.repeat(22), trackB = 'F'.repeat(22), calls = [], base = fixtureFetch(calls);
  const fetchImpl = async (url, options = {}) => {
    if (new URL(url).pathname === '/pathfinder/v2/query' && JSON.parse(options.body).operationName === 'getAlbum') {
      calls.push({ target: new URL(url).href, options });
      const request = JSON.parse(options.body), offset = request.variables.offset;
      const rows = offset === 0
        ? Array.from({ length: 50 }, (_, index) => ({ track: { __typename: 'Track', uri: `spotify:track:${index === 0 ? trackA : String(index).padStart(22, '0')}`, name: `Track ${index}`, artists: { items: [{ profile: { name: 'Fixture Artist' } }] }, duration: { totalMilliseconds: index === 0 ? 183250 : 181125 }, albumOfTrack: { name: 'Fixture album', uri: `spotify:album:${albumId}`, coverArt: { sources: [{ url: 'https://i.scdn.co/image/album' }] } }, discNumber: 1, trackNumber: index + 1, playability: { playable: true }, associationsV3: { videoAssociations: { totalCount: 0 } } } }))
        : [{ track: { __typename: 'Track', uri: `spotify:track:${trackB}`, name: 'Last track', artists: { items: [{ profile: { name: 'Fixture Artist' } }] }, duration: { totalMilliseconds: 181125 }, albumOfTrack: { name: 'Fixture album', uri: `spotify:album:${albumId}` }, discNumber: 1, trackNumber: 51, playability: { playable: true }, associationsV3: { videoAssociations: { totalCount: 0 } } } }];
      return Response.json({ data: { albumUnion: { __typename: 'Album', uri: `spotify:album:${albumId}`, tracksV2: { totalCount: 51, items: rows } } } });
    }
    return base(url, options);
  };
  const job = cursor => executeJob({ provider: 'spotify', operation: 'getAlbumTracks', args: { albumId, limit: 50, ...(cursor ? { cursor } : {}) }, cookieHeader: SECRET_COOKIE }, { fetchImpl });
  const first = await job(null);assert.equal(first.ok, true, JSON.stringify(first));assert.equal(first.result.items.length, 50);assert.equal(first.result.items[0].trackId, trackA);assert.equal(first.result.items[0].durationSeconds, 183.25);assert.equal(first.result.total, 51);assert.equal(first.result.nextCursor, '50');assert.equal(Object.hasOwn(first.result, 'pagingQualification'), false);
  const second = await job(first.result.nextCursor);assert.equal(second.ok, true, JSON.stringify(second));assert.equal(second.result.items[0].trackId, trackB);assert.equal(second.result.pagingQualification, 'experimental-offset');assert.equal(second.result.nextCursor, null);
  const requests = calls.filter(call => new URL(call.target).pathname === '/pathfinder/v2/query' && JSON.parse(call.options.body).operationName === 'getAlbum').map(call => JSON.parse(call.options.body));
  assert.deepEqual(requests.map(request => [request.variables.offset, request.variables.limit]), [[0, 50], [50, 50]]);
  assert.ok(requests.every(request => request.variables.uri === `spotify:album:${albumId}` && request.variables.locale === ''));
  const rows = [...first.result.items, ...second.result.items];
  assert.deepEqual(rows.map(row => row.position), Array.from({ length: 51 }, (_, index) => index + 1));
  assert.equal(new Set(rows.map(row => row.trackId)).size, 51);
  assert.ok(rows.every(row => Number.isFinite(row.durationSeconds) && row.durationSeconds > 0));
  assert.ok(requests.every(request => request.extensions.persistedQuery.sha256Hash === '11c9cad9cb24c4f9e25ef9456f68621cfd6006a90f679e729c1d2f2fd70dfe16'));
  assert.equal(JSON.stringify(first).includes(SECRET_COOKIE), false);assert.equal(JSON.stringify(second).includes('TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture'), false);
});


test('catalog GraphQL errors expose only a vetted artist or album operation name', async () => {
  const responseMarker = 'private-provider-error-fixture';
  for (const [operationName, operation, args] of [
    ['queryArtistMinimal', 'getArtist', { artistId: 'A'.repeat(22) }],
    ['queryArtistDiscographyAll', 'getArtistAlbums', { artistId: 'A'.repeat(22), limit: 20 }],
    ['getAlbum', 'getAlbum', { albumId: 'B'.repeat(22) }],
    ['getAlbum', 'getAlbumTracks', { albumId: 'B'.repeat(22), limit: 50 }],
  ]) {
    const calls = [], base = fixtureFetch(calls);
    const fetchImpl = async (url, options = {}) => {
      if (new URL(url).pathname === '/pathfinder/v2/query') {
        const request = JSON.parse(options.body || '{}');
        if (request.operationName === operationName) {
          calls.push({ target: new URL(url).href, options });
          return Response.json({ errors: [{ message: responseMarker, extensions: { authorization: 'Bearer fixture-provider-token', cookie: SECRET_COOKIE, url: 'https://private.example/request' } }] });
        }
      }
      return base(url, options);
    };
    const result = await executeJob({ provider: 'spotify', operation, args, cookieHeader: SECRET_COOKIE }, { fetchImpl });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'PROVIDER_CHANGED');
    assert.equal(result.error.phase, 'catalog_graphql');
    assert.equal(result.error.httpStatus, 200);
    assert.equal(result.error.operationName, operationName);
    assert.equal(calls.filter(call => JSON.parse(call.options.body || '{}').operationName === operationName).length, 1);
    const serialized = JSON.stringify(result);
    for (const privateValue of [SECRET_COOKIE, responseMarker, 'fixture-provider-token', 'private.example']) assert.equal(serialized.includes(privateValue), false);
  }
});

test('Spotify high-level apply refuses an invalid stored preview before dispatching any mutation', async () => {
  const calls = [];
  const result = await executeJob({
    provider: 'spotify', operation: 'applySyncPreview', cookieHeader: SECRET_COOKIE,
    args: {
      previewApproved: true, playlistId: '1234567890123456789012', expectedEntries: [],
      removeEntryIds: [], addTrackUris: [], desiredTrackUris: [],
      syncPreview: { previewId: 'bad', accountId: 'fixture-account', baselineRevision: '', remoteRevision: '', remoteHash: 'invalid' },
    },
  }, { fetchImpl: async (...args) => { calls.push(args); return fixtureFetch([])(...args); } });
  assert.equal(result.ok, false);
  assert.equal(calls.some(([url, options]) => new URL(url).pathname.includes('pathfinder') && JSON.parse(options.body || '{}').operationName !== 'profileAttributes'), false);
  assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
});

test('Spotify high-level apply reports an unknown outcome after one dispatched mutation failure', async () => {
  const playlistId = 'P'.repeat(22), firstTrack = 'A'.repeat(22), secondTrack = 'B'.repeat(22), calls = [];
  const fetchImpl = async (url, options = {}) => {
    const target = new URL(url).href;
    calls.push({ target, options });
    if (target === 'https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef') return Response.json({ files: { 'registry.json': { content: JSON.stringify([{ s: 'JBSWY3DPEHPK3PXP', v: 61 }]) } } });
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://open.spotify.com/api/server-time') return Response.json({ serverTime: 100000 });
    if (new URL(target).pathname === '/api/token') return Response.json({ accessToken: 'TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture', isAnonymous: false });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') {
      const request = JSON.parse(options.body);
      if (request.operationName === 'profileAttributes') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account', name: 'Fixture Account' } } } });
      if (request.operationName === 'fetchPlaylist') return Response.json({ data: { playlistV2: {
        id: playlistId, uri: `spotify:playlist:${playlistId}`, name: 'Fixture playlist', ownerV2: { data: { id: 'fixture-account', name: 'Fixture Account' } },
        content: { totalCount: 1, items: [{ uid: 'occurrence-1', itemV2: { data: { id: firstTrack, uri: `spotify:track:${firstTrack}`, name: 'First', artists: [{ name: 'Artist' }], albumOfTrack: { name: 'Album' } } } }] },
      } } });
      if (request.operationName === 'addToPlaylist') throw new Error('connection lost after dispatch');
      throw new Error('unexpected GraphQL operation');
    }
    throw new Error('unexpected fixture URL');
  };
  const result = await executeJob({ provider: 'spotify', operation: 'applySyncPreview', cookieHeader: SECRET_COOKIE, args: {
    previewApproved: true, playlistId,
    expectedEntries: [{ entryId: 'occurrence-1', trackId: firstTrack }],
    removeEntryIds: [], addTrackUris: [`spotify:track:${secondTrack}`],
    desiredTrackUris: [`spotify:track:${firstTrack}`, `spotify:track:${secondTrack}`],
    syncPreview: { previewId: 'fixture-preview-1', accountId: 'fixture-account', baselineRevision: 'base', remoteRevision: 'remote', remoteHash: 'a'.repeat(64) },
  } }, { fetchImpl });
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'OUTCOME_UNKNOWN');
  assert.match(result.error.message, /Refresh the playlist/);
  assert.equal(calls.filter(call => call.target === 'https://api-partner.spotify.com/pathfinder/v2/query' && JSON.parse(call.options.body).operationName === 'addToPlaylist').length, 1);
  assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
});

test('Spotify high-level apply verifies the complete final playlist before returning success', async () => {
  const playlistId = 'P'.repeat(22), firstTrack = 'A'.repeat(22), secondTrack = 'B'.repeat(22), calls = [];
  const tracks = [firstTrack];
  const playlistBody = () => ({ data: { playlistV2: {
    id: playlistId, uri: `spotify:playlist:${playlistId}`, name: 'Fixture playlist', ownerV2: { data: { id: 'fixture-account', name: 'Fixture Account' } },
    content: { totalCount: tracks.length, items: tracks.map((id, index) => ({ uid: `occurrence-${index + 1}`, itemV2: { data: { id, uri: `spotify:track:${id}`, name: `Track ${index + 1}`, artists: [{ name: 'Artist' }], albumOfTrack: { name: 'Album' } } } })) },
  } } });
  const fetchImpl = async (url, options = {}) => {
    const target = new URL(url).href;
    calls.push({ target, options });
    if (target === 'https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef') return Response.json({ files: { 'registry.json': { content: JSON.stringify([{ s: 'JBSWY3DPEHPK3PXP', v: 61 }]) } } });
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://open.spotify.com/api/server-time') return Response.json({ serverTime: 100000 });
    if (new URL(target).pathname === '/api/token') return Response.json({ accessToken: 'TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture', isAnonymous: false });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') {
      const request = JSON.parse(options.body);
      if (request.operationName === 'profileAttributes') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account', name: 'Fixture Account' } } } });
      if (request.operationName === 'fetchPlaylist') return Response.json(playlistBody());
      if (request.operationName === 'addToPlaylist') {
        tracks.push(...request.variables.playlistItemUris.map(uri => uri.slice('spotify:track:'.length)));
        return Response.json({ data: { addToPlaylist: { status: 'SUCCESS' } } });
      }
      throw new Error('unexpected GraphQL operation');
    }
    throw new Error('unexpected fixture URL');
  };
  const result = await executeJob({ provider: 'spotify', operation: 'applySyncPreview', cookieHeader: SECRET_COOKIE, args: {
    previewApproved: true, playlistId,
    expectedEntries: [{ entryId: 'occurrence-1', trackId: firstTrack }],
    removeEntryIds: [], addTrackUris: [`spotify:track:${secondTrack}`],
    desiredTrackUris: [`spotify:track:${firstTrack}`, `spotify:track:${secondTrack}`],
    syncPreview: { previewId: 'fixture-preview-1', accountId: 'fixture-account', baselineRevision: 'base', remoteRevision: 'remote', remoteHash: 'a'.repeat(64) },
  } }, { fetchImpl });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(JSON.parse(JSON.stringify(result.result)), { accountId: 'fixture-account', playlistId, applied: 1, status: 'VERIFIED' });
  assert.equal(tracks.length, 2);
  assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
  assert.equal(JSON.stringify(result).includes('TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture'), false);
});

test('rejects browser APIs, malformed cookies, and unsupported worker operations without echoing input', async () => {
  const fetchImpl = async () => { throw new Error('must not fetch'); };
  for (const job of [
    { provider: 'spotify', operation: 'status', args: {}, cookieHeader: SECRET_COOKIE },
    { provider: 'youtube', operation: 'search', args: {}, cookieHeader: SECRET_COOKIE },
    { provider: 'spotify', operation: 'addTracks', args: {}, cookieHeader: SECRET_COOKIE },
    { provider: 'spotify', operation: 'status', args: {}, cookieHeader: 'sp_dc=secret\r\nAuthorization: Bearer leak' },
    { provider: 'spotify', operation: 'status', args: {}, cookieHeader: 'sp_dc=secret; evil=leak' },
  ]) {
    const result = await executeJob(job, { fetchImpl });
    assert.equal(result.ok, false);
    assert.equal(JSON.stringify(result).includes('secret'), false);
    assert.equal(JSON.stringify(result).includes('leak'), false);
  }
});

test('errors identify only the failed Spotify phase and HTTP status', async () => {
  const base = async (url, options = {}) => {
    const target = new URL(url).href;
    if (target === 'https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef') return Response.json({ files: { 'registry.json': { content: JSON.stringify([{ s: 'JBSWY3DPEHPK3PXP', v: 61 }]) } } });
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://open.spotify.com/api/server-time') return Response.json({ serverTime: 100000 });
    if (new URL(target).pathname === '/api/token') return new Response(JSON.stringify({ error: 'denied' }), { status: 429, headers: { 'retry-after': '3' } });
    throw new Error(`unexpected request ${target}`);
  };
  const token = await executeJob({ provider: 'spotify', operation: 'status', args: {}, cookieHeader: SECRET_COOKIE }, { fetchImpl: base });
  assert.equal(token.ok, false);
  assert.equal(token.error.phase, 'token_exchange');
  assert.equal(token.error.httpStatus, 429);
  assert.equal(token.error.retryAfterSeconds, 3);
  assert.equal(Object.hasOwn(token.error, 'operationName'), false);
  assert.match(token.error.message, /phase=token_exchange, HTTP 429.*retry-after=3s/);
  assert.equal(JSON.stringify(token).includes('denied'), false);

  let ownerStatus = 403;
  const profileFetch = async (url, options = {}) => {
    const target = new URL(url).href;
    if (target === 'https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef') return Response.json({ files: { 'registry.json': { content: JSON.stringify([{ s: 'JBSWY3DPEHPK3PXP', v: 61 }]) } } });
    if (target === 'https://francescograzioso.github.io/Meld/spotify-gql-hashes.json') return Response.json({ operations: {} });
    if (target === 'https://open.spotify.com/api/server-time') return Response.json({ serverTime: 100000 });
    if (new URL(target).pathname === '/api/token') return Response.json({ accessToken: 'TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture', isAnonymous: false });
    if (target === 'https://api-partner.spotify.com/pathfinder/v2/query') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account', name: 'Fixture' } } } });
    if (target.startsWith('https://spclient.wg.spotify.com/user-profile-view/v3/profile/')) return new Response('{"error":"private profile"}', { status: ownerStatus, headers: ownerStatus === 429 ? { 'retry-after': '22' } : {} });
    throw new Error('unexpected request');
  };
  const profile = await executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'spotify' }, cookieHeader: SECRET_COOKIE }, { fetchImpl: profileFetch });
  assert.equal(profile.ok, false);
  assert.equal(profile.error.phase, 'owner_profile');
  assert.equal(profile.error.httpStatus, 403);
  assert.equal(Object.hasOwn(profile.error, 'operationName'), false);
  assert.match(profile.error.message, /phase=owner_profile, HTTP 403/);
  assert.equal(JSON.stringify(profile).includes('private profile'), false);

  ownerStatus = 429;
  const limited = await executeJob({ provider: 'spotify', operation: 'listPlaylists', args: { ownerId: 'spotify' }, cookieHeader: SECRET_COOKIE }, { fetchImpl: profileFetch });
  assert.equal(limited.ok, false);
  assert.equal(limited.error.phase, 'owner_profile');
  assert.equal(limited.error.httpStatus, 429);
  assert.equal(limited.error.retryAfterSeconds, 22);
  assert.equal(limited.error.message, 'Spotify temporarily limited profile browsing. Try again in 22 seconds.');
});

test('staged native worker resolves its packaged scripts and executes with fixture HTTP', async () => {
  const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const stagedRoot = await mkdtemp(path.join(os.tmpdir(), 'burrata-native-worker-fixture-'));
  try {
    await mkdir(path.join(stagedRoot, 'scripts'), { recursive: true });
    await mkdir(path.join(stagedRoot, 'extension'), { recursive: true });
    await copyFile(path.join(projectRoot, 'scripts/provider-session-worker.mjs'), path.join(stagedRoot, 'scripts/provider-session-worker.mjs'));
    await copyFile(path.join(projectRoot, 'extension/spotify-auth.js'), path.join(stagedRoot, 'extension/spotify-auth.js'));
    await copyFile(path.join(projectRoot, 'extension/provider-operations.js'), path.join(stagedRoot, 'extension/provider-operations.js'));
    const stagedUrl = pathToFileURL(path.join(stagedRoot, 'scripts/provider-session-worker.mjs')).href;
    const { executeJob: runStagedJob } = await import(`${stagedUrl}?fixture=staged`);
    const result = await runStagedJob(
      { provider: 'spotify', operation: 'search', args: { query: 'Fixture', kind: 'track', limit: 30 }, cookieHeader: SECRET_COOKIE },
      { fetchImpl: fixtureFetch([]) },
    );
    assert.equal(result.ok, true);
    assert.equal(result.result.items[0].title, 'Fixture Song');
    assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
  } finally {
    await rm(stagedRoot, { recursive: true, force: true });
  }
});

test('native Spotify worker distinguishes a catalog timeout from provider throttling', async () => {
  for (const timedOut of [true, false]) {
    const base = fixtureFetch([]);
    const result = await executeJob({provider:'spotify',operation:'getTrack',args:{trackId:'1234567890123456789012'},cookieHeader:SECRET_COOKIE}, {
      fetchImpl: async (url, options) => {
        if (new URL(url).pathname === '/v1/tracks/1234567890123456789012') {
          if (timedOut) throw new DOMException('Fixture timeout', 'AbortError');
          return new Response('{}', {status:429,headers:{'retry-after':'36'}});
        }
        return base(url, options);
      }
    });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, timedOut ? 'REQUEST_TIMEOUT' : 'RATE_LIMITED');
    assert.equal(result.error.phase, 'catalog_rest');
    assert.equal(result.error.httpStatus, timedOut ? undefined : 429);
    assert.equal(result.error.retryAfterSeconds, timedOut ? undefined : 36);
    assert.equal(JSON.stringify(result).includes(SECRET_COOKIE), false);
  }
});

const VERIFICATION_ID = '1234567890123456789012';
const verificationRaw = () => ({ id: VERIFICATION_ID, name: 'Burrata verification', public: false, collaborative: false, owner: { id: 'fixture-account' }, tracks: { total: 0, items: [], next: null } });
function verificationFetch(transform = value => value) {
  const calls = [], base = fixtureFetch(calls);
  return { calls, fetchImpl: async (url, options) => {
    const parsed = new URL(url);
    if (parsed.hostname === 'api.spotify.com' && parsed.pathname === '/v1/me/playlists') {
      calls.push({ target: parsed.href, options });
      assert.equal(options.method, 'POST');
      assert.deepEqual(JSON.parse(options.body), { name: 'Burrata verification', public: false, collaborative: false });
      return Response.json(transform(verificationRaw()));
    }
    if (parsed.hostname === 'api.spotify.com' && parsed.pathname === `/v1/playlists/${VERIFICATION_ID}`) {
      calls.push({ target: parsed.href, options });
      assert.equal(options.method || 'GET', 'GET');
      return Response.json(transform(verificationRaw()));
    }
    return base(url, options);
  } };
}
test('fixed Spotify private empty creation and fresh API readback prove owner and contents', async () => {
  const fixture = verificationFetch();
  const common = { provider: 'spotify', cookieHeader: SECRET_COOKIE };
  const created = await executeJob({ ...common, creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist', args: { accountId: 'fixture-account' } }, fixture);
  assert.equal(created.ok, true, JSON.stringify(created));
  assert.equal(created.result.playlistId, VERIFICATION_ID);
  const verified = await executeJob({ ...common, operation: 'verifyVerificationPlaylist', args: { accountId: 'fixture-account', playlistId: VERIFICATION_ID } }, fixture);
  assert.equal(verified.ok, true, JSON.stringify(verified));
  assert.equal(verified.result.playlist.ownerId, 'fixture-account');
  assert.equal(verified.result.playlist.visibility, 'private');
  assert.equal(verified.result.complete, true);
  assert.equal(fixture.calls.filter(call => call.options.method === 'POST' && new URL(call.target).hostname === 'api.spotify.com').length, 1);
});
test('Spotify creation forbids caller overrides and unknown outcomes forbid retries', async () => {
  const common = { provider: 'spotify', cookieHeader: SECRET_COOKIE, creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist' };
  for (const args of [{ accountId: 'wrong-account' }, { accountId: 'fixture-account', public: true }, { accountId: 'fixture-account', name: 'Other' }]) {
    const fixture = verificationFetch();
    const result = await executeJob({ ...common, args }, fixture);
    assert.equal(result.ok, false);
    assert.equal(fixture.calls.some(call => new URL(call.target).hostname === 'api.spotify.com'), false);
  }
  for (const patch of [{ id: 'bad-id' }]) {
    const fixture = verificationFetch(raw => ({ ...raw, ...patch }));
    const result = await executeJob({ ...common, args: { accountId: 'fixture-account' } }, fixture);
    assert.equal(result.error.code, 'OUTCOME_UNKNOWN', JSON.stringify(result));
    assert.match(result.error.message, /Do not retry creation/);
    assert.equal(fixture.calls.filter(call => new URL(call.target).hostname === 'api.spotify.com').length, 1);
  }
});
test('Spotify verification rejects wrong owner, public, nonempty or incomplete fresh API readbacks', async () => {
  for (const patch of [{ owner: { id: 'wrong-owner' } }, { public: true }, { tracks: { total: 0, items: [], next: 'more' } }, { tracks: { total: 1, items: [] } }]) {
    const fixture = verificationFetch(raw => ({ ...raw, ...patch }));
    const result = await executeJob({ provider: 'spotify', cookieHeader: SECRET_COOKIE, operation: 'verifyVerificationPlaylist', args: { accountId: 'fixture-account', playlistId: VERIFICATION_ID } }, fixture);
    assert.equal(result.ok, false);
  }
});


test('Spotify creation retains returned ID for required fresh verification despite renamed item schema', async () => {
  const fixture = verificationFetch(raw => { const { tracks, ...rest } = raw; return { ...rest, items: tracks }; });
  const common = { provider: 'spotify', cookieHeader: SECRET_COOKIE };
  const created = await executeJob({ ...common, creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist', args: { accountId: 'fixture-account' } }, fixture);
  assert.equal(created.ok, true, JSON.stringify(created));
  assert.equal(created.result.status, 'CREATED_REQUIRES_READBACK');
  assert.equal(created.result.playlistId, VERIFICATION_ID);
  assert.equal(created.result.visibility, undefined);
  const verified = await executeJob({ ...common, operation: 'verifyVerificationPlaylist', args: { accountId: 'fixture-account', playlistId: VERIFICATION_ID } }, fixture);
  assert.equal(verified.ok, true, JSON.stringify(verified));
  assert.equal(verified.result.playlist.ownerId, 'fixture-account');
  assert.equal(verified.result.complete, true);
});
test('Spotify modern item schema still rejects public, wrong owner and nonempty pages', async () => {
  for (const patch of [{ public: true }, { owner: { id: 'other-owner' } }, { items: { total: 1, items: [], next: null } }]) {
    const fixture = verificationFetch(raw => { const { tracks, ...rest } = raw; return { ...rest, items: tracks, ...patch }; });
    const verified = await executeJob({ provider: 'spotify', cookieHeader: SECRET_COOKIE, operation: 'verifyVerificationPlaylist', args: { accountId: 'fixture-account', playlistId: VERIFICATION_ID } }, fixture);
    assert.equal(verified.ok, false, JSON.stringify(verified));
  }
});


test('fixed creation uses only current-account endpoint and preserves HTTP metadata through real worker executor', async () => {
  for (const [path, allowed] of [['/v1/me/playlists',true],['/v1/users/fixture-account/playlists',false],['/v1/me/playlists/extra',false],['/v1/me/playlists?public=true',false]]) assert.equal(allowedRequest(`https://api.spotify.com${path}`,'POST'),allowed);
  for (const status of [401, 403, 429, 500]) {
    const fixture = verificationFetch(), base = fixture.fetchImpl;
    let writes = 0;
    fixture.fetchImpl = async (url, options) => {
      if (new URL(url).pathname !== '/v1/me/playlists') return base(url, options);
      writes++;
      return Response.json({ error: { message: 'private provider denial and TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture', status } }, { status });
    };
    const result = await executeJob({ provider:'spotify',cookieHeader:SECRET_COOKIE,creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:{accountId:'fixture-account'} }, fixture);
    assert.equal(writes,1);
    assert.equal(result.error.code,'OUTCOME_UNKNOWN',JSON.stringify(result));
    assert.equal(result.error.httpStatus,status);
    assert.equal(result.error.phase,'playlist_create');
    assert.match(result.error.message,/Do not retry creation/);
    assert.doesNotMatch(JSON.stringify(result),/private provider denial|TEST-FIXTURE-CREDENTIAL-67AE8876BC-sensitive-access-token-fixture|fixture-cookie-secret/);
  }
});

test('v5 profile windows advance by the requested span when source slots omit a row', async()=>{
 const offsets=[],seen=new Set();
 const run=cursor=>executeJob({provider:'spotify',operation:'listPlaylists',args:{ownerId:'owner123',limit:30,...(cursor?{cursor}:{})},cookieHeader:SECRET_COOKIE,cachedAuth:{accessToken:'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture',expiresAt:Date.now()+60000}},{fetchImpl:async url=>{
  const target=new URL(url);
  if(target.href==='https://francescograzioso.github.io/Meld/spotify-gql-hashes.json')return Response.json({operations:{}});
  if(target.hostname==='api-partner.spotify.com')return Response.json({data:{me:{profile:{uri:'spotify:user:fixture-account',name:'Fixture'}}}});
  if(target.hostname==='spclient.wg.spotify.com'){
   const offset=Number(target.searchParams.get('offset')),limit=Number(target.searchParams.get('limit'));offsets.push(offset);
   const rows=Array.from({length:Math.min(limit,Math.max(0,100-offset))},(_,i)=>offset+i).filter(index=>index!==75).map(index=>protoPlaylist(index));
   return new Response(new Uint8Array([...rows.flatMap(row=>protoField(1,2,[...protoVarint(row.length),...row])),...protoInt(2,110)]),{headers:{'content-type':'application/x-protobuf'}});
  }
  throw Error('unexpected fixture URL');
 }});
 let cursor=null,result;const counts=[];
 for(let page=0;page<5;page++){
  result=await run(cursor);assert.equal(result.ok,true,JSON.stringify(result));counts.push(result.result.items.length);
  for(const row of result.result.items){assert.equal(seen.has(row.id),false);seen.add(row.id);}
  cursor=result.result.nextCursor;if(!cursor)break;
 }
 assert.deepEqual(counts,[30,30,29,10,0]);assert.equal(seen.size,99);
 assert.deepEqual(offsets,[0,0,30,30,60,60,90,90,120]);
 assert.equal(result.result.totalCount,110);assert.equal(result.result.complete,false);assert.equal(result.result.truncated,true);assert.equal(cursor,null);
});

test('creation receipt proves predispatch rejection without leaking account or secrets', async () => {
  const fixture = verificationFetch();
  const result = await executeJob({ provider:'spotify',cookieHeader:SECRET_COOKIE,creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:{accountId:'wrong-account'} }, fixture);
  assert.equal(result.ok, false);
  const receipt=result.error.creationReceipt;
  assert.equal(receipt.mutationDispatched,false);assert.equal(receipt.playlistId,null);
  assert.match(receipt.requestId,/^[a-f0-9]{32}$/);
  assert.doesNotMatch(JSON.stringify(receipt),/fixture-account|wrong-account|cookie|Bearer|https?:/i);
  assert.equal(fixture.calls.some(call=>new URL(call.target).pathname==='/v1/me/playlists'),false);
});
test('creation receipt retains true dispatch after simulated postdispatch timeout', async () => {
  const fixture=verificationFetch(),base=fixture.fetchImpl;let writes=0;
  fixture.fetchImpl=async(url,options)=>{if(new URL(url).pathname==='/v1/me/playlists'){writes++;throw new DOMException('private timeout detail','AbortError');}return base(url,options);};
  const result=await executeJob({provider:'spotify',cookieHeader:SECRET_COOKIE,creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:{accountId:'fixture-account'}},fixture);
  assert.equal(result.error.code,'OUTCOME_UNKNOWN');assert.equal(result.error.creationReceipt.mutationDispatched,true);
  assert.equal(result.error.creationReceipt.playlistId,null);assert.equal(writes,1);
  assert.doesNotMatch(JSON.stringify(result),/private timeout detail/);
});
test('successful creation receipt binds bounded ID and unique worker request', async () => {
  const input={provider:'spotify',cookieHeader:SECRET_COOKIE,creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:{accountId:'fixture-account'}};
  const first=await executeJob(input,verificationFetch()),second=await executeJob({...input,creationRequestId:'abcdef0123456789abcdef0123456789'},verificationFetch());
  assert.equal(first.ok,true);assert.equal(first.result.creationReceipt.playlistId,VERIFICATION_ID);
  assert.equal(first.result.creationReceipt.mutationDispatched,true);
  assert.notEqual(first.result.creationReceipt.requestId,second.result.creationReceipt.requestId);
  const malformed=await executeJob(input,verificationFetch(raw=>({...raw,id:'bad'})));
  assert.equal(malformed.ok,false);assert.equal(malformed.error.creationReceipt.playlistId,null);
});

test('creation rejects a missing or malformed native request ID before any network dispatch', async () => {
  for (const requestId of [undefined, 'private-cookie-value', 'ABCDEF0123456789ABCDEF0123456789']) {
    let calls=0;
    const input={provider:'spotify',cookieHeader:SECRET_COOKIE,operation:'createVerificationPlaylist',args:{accountId:'fixture-account'}};
    if(requestId!==undefined) input.creationRequestId=requestId;
    const result=await executeJob(input,{fetchImpl:async()=>{calls++;throw new Error('must not dispatch');}});
    assert.equal(result.ok,false);assert.equal(calls,0);assert.equal(result.error.creationReceipt,undefined);
    assert.doesNotMatch(JSON.stringify(result),/private-cookie-value/);
  }
});

test('creation dispatch is false for expired deadline, aborted signal and invalid bearer before fetch', async () => {
  const url='https://api.spotify.com/v1/me/playlists';
  const body=JSON.stringify({name:'Burrata verification',public:false,collaborative:false});
  for(const gate of ['deadline','aborted','bearer']){
    const controller=new AbortController();if(gate==='aborted')controller.abort();
    let mutationDispatched=false,mutationFetchCalls=0;
    const fetch=makeFetcher(SECRET_COOKIE,gate==='deadline'?Date.now()-1:Date.now()+5000,async()=>{mutationFetchCalls++;return Response.json({});},null,'fixture-account',()=>{mutationDispatched=true;});
    await assert.rejects(fetch(url,{method:'POST',body,headers:{Authorization:gate==='bearer'?'bad':'Bearer fixture-access-token'},signal:controller.signal}));
    assert.equal(mutationDispatched,false,gate);assert.equal(mutationFetchCalls,0,gate);
  }
});
test('creation dispatch is true when fetch itself throws after final local gates', async () => {
  let mutationDispatched=false,mutationFetchCalls=0;
  const fetch=makeFetcher(SECRET_COOKIE,Date.now()+5000,async()=>{mutationFetchCalls++;throw new Error('postdispatch failure');},null,'fixture-account',()=>{mutationDispatched=true;});
  await assert.rejects(fetch('https://api.spotify.com/v1/me/playlists',{method:'POST',body:JSON.stringify({name:'Burrata verification',public:false,collaborative:false}),headers:{Authorization:'Bearer fixture-access-token'}}));
  assert.equal(mutationDispatched,true);assert.equal(mutationFetchCalls,1);
});

// Synthetic wire rows exercise diagnostic fidelity only; they do not establish provider offset semantics.
async function inspectHashRows(indices, total=110, offset=58, limit=30, idOverrides={}) {
  const rows=indices.map((index,i)=>protoPlaylist(index,idOverrides[i]));
  const bytes=new Uint8Array([...rows.flatMap(row=>protoField(1,2,[...protoVarint(row.length),...row])),...protoInt(2,total)]);
  const result=await executeJob({provider:'spotify',operation:'listPlaylists',args:{ownerId:'owner123',inspectProfileResponse:true,offset,limit},cookieHeader:SECRET_COOKIE,cachedAuth:{accessToken:'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture',expiresAt:Date.now()+60000}},{fetchImpl:async url=>{
    const target=new URL(url);
    if(target.hostname==='francescograzioso.github.io')return Response.json({operations:{}});
    if(target.hostname==='api-partner.spotify.com')return Response.json({data:{me:{profile:{uri:'spotify:user:fixture-account'}}}});
    return new Response(bytes,{headers:{'content-type':'application/x-protobuf'}});
  }});
  assert.equal(result.ok,true,JSON.stringify(result));return JSON.parse(JSON.stringify(result.result.profileResponseShape));
}
function protoSummaryPlaylist(index) {
  const id=String(index+1).padStart(22,'0');
  return [...protoString(1,`spotify:playlist:${id}`),...protoString(2,`Fixture ${index+1}`),...protoString(3,`https://i.scdn.co/image/fixture-${index+1}`),...protoInt(4,index*10),...protoString(5,'Fixture Owner'),...protoString(6,'spotify:user:owner123'),...protoInt(7,1)];
}
async function inspectProfileSummaryHashRows(indices,total=110,playlistLimit=200,bodyOverride=null,contentType='application/x-protobuf'){
  // Match the pinned WebPlayer Playlist protobuf fields 1-7; the generic
  // paged helper also adds a deprecated group used by separate parser tests.
  const rows=indices.map(protoSummaryPlaylist);
  const bytes=new Uint8Array([...protoString(1,'spotify:user:owner123'),...rows.flatMap(row=>protoField(8,2,[...protoVarint(row.length),...row])),...(total===0?[]:protoInt(9,total))]);const responseBody=bodyOverride||bytes;
  let requestedUrl='';
  const result=await executeJob({provider:'spotify',operation:'listPlaylists',args:{ownerId:'owner123',inspectProfileResponse:true,profileSummary:true,playlistLimit},cookieHeader:SECRET_COOKIE,cachedAuth:{accessToken:'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture',expiresAt:Date.now()+60000}},{fetchImpl:async url=>{
    const target=new URL(url);if(target.hostname==='francescograzioso.github.io')return Response.json({operations:{}});if(target.hostname==='api-partner.spotify.com')return Response.json({data:{me:{profile:{uri:'spotify:user:fixture-account'}}}});requestedUrl=target.href;return new Response(responseBody,{headers:{'content-type':contentType}});
  }});
  assert.equal(result.ok,true,JSON.stringify(result));return {shape:JSON.parse(JSON.stringify(result.result.profileResponseShape)),requestedUrl};
}
test('profile summary inspector uses the pinned full-profile endpoint and exposes only ordered hashes',async()=>{
  const inspected=await inspectProfileSummaryHashRows(Array.from({length:99},(_,index)=>index));
  const url=new URL(inspected.requestedUrl);assert.equal(url.pathname,'/user-profile-view/v3/profile/owner123');assert.equal(url.searchParams.get('playlist_limit'),'200');assert.equal(url.searchParams.get('artist_limit'),'0');assert.equal(url.searchParams.get('episode_limit'),'0');assert.equal(url.searchParams.get('market'),'from_token');
  assert.equal(inspected.shape.responseMode,'summary');assert.equal(inspected.shape.parseClass,'valid');assert.equal(inspected.shape.failureClass,'page_count');assert.equal(inspected.shape.decodedRows,99);assert.equal(inspected.shape.decodedTotal,110);assert.equal(inspected.shape.playlistIdHashes.length,99);
  assert.doesNotMatch(JSON.stringify(inspected.shape),/owner123|fixture-account|fixture-cookie|spotify:playlist|https?:/i);
  const empty=await inspectProfileSummaryHashRows([],0);assert.equal(empty.shape.parseClass,'valid');assert.equal(empty.shape.failureClass,'none');assert.equal(empty.shape.decodedRows,0);assert.equal(empty.shape.decodedTotal,0);
  const malformed=await inspectProfileSummaryHashRows([],0,200,new Uint8Array([...protoString(1,'spotify:user:owner123'),...protoInt(8,1)]));assert.equal(malformed.shape.parseClass,'malformed');assert.equal(malformed.shape.failureClass,'response_shape');
  const json=await inspectProfileSummaryHashRows([],0,200,new TextEncoder().encode('{"error":"fixture"}'),'application/json');assert.equal(json.shape.parseClass,'not_binary');assert.equal(json.shape.failureClass,'content_type');
  const tooLarge=await inspectProfileSummaryHashRows([],0,200,new Uint8Array(2*1024*1024+1));assert.equal(tooLarge.shape.parseClass,'oversize');assert.equal(tooLarge.shape.bodyBytes,2*1024*1024);
  let oversizedDispatches=0;const oversized=await executeJob({provider:'spotify',operation:'listPlaylists',args:{ownerId:'owner123',inspectProfileResponse:true,profileSummary:true,playlistLimit:201},cookieHeader:SECRET_COOKIE,cachedAuth:{accessToken:'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture',expiresAt:Date.now()+60000}},{fetchImpl:async url=>{if(new URL(url).pathname.startsWith('/user-profile-view/'))oversizedDispatches++;throw new Error('unexpected request');}});
  assert.equal(oversized.ok,false);assert.equal(oversizedDispatches,0);
});
test('profile inspector ordered hashes retain sparse-page overlap despite page-count failure',async()=>{
  const left=await inspectHashRows([58,59,61,62]);
  const right=await inspectHashRows([59,61,62,63],109,59);
  assert.equal(left.failureClass,'page_count');assert.equal(left.playlistIdHashes.length,4);
  assert.deepEqual(left.playlistIdHashes.slice(1),right.playlistIdHashes.slice(0,3));
  const id=String(59).padStart(22,'0');
  const expected=Buffer.from(await webcrypto.subtle.digest('SHA-256',new TextEncoder().encode(`burrata:spotify:playlist-id:v1:${id}`))).toString('hex');
  assert.equal(left.playlistIdHashes[0],expected);
  assert.notEqual(left.playlistIdHashes[0],right.playlistIdHashes[0]);
  assert.doesNotMatch(JSON.stringify(left),/owner123|fixture-account|fixture-cookie|spotify:playlist|https?:/i);
});
test('profile inspector hashes preserve duplicate order and omit malformed IDs',async()=>{
  const duplicate=await inspectHashRows([0,0],2,0);
  assert.equal(duplicate.failureClass,'duplicate_id');assert.equal(duplicate.playlistIdHashes.length,2);
  assert.equal(duplicate.playlistIdHashes[0],duplicate.playlistIdHashes[1]);
  const malformed=await inspectHashRows([0,1],2,0,30,{1:'malformed-private-id'});
  assert.equal(malformed.failureClass,'invalid_id');assert.deepEqual(malformed.playlistIdHashes,[]);
});
test('profile inspector hash output caps at thirty rows without truncating evidence',async()=>{
  const oversized=await inspectHashRows(Array.from({length:31},(_,i)=>i),110,0);
  assert.equal(oversized.decodedRows,31);assert.equal(oversized.failureClass,'page_count');
  assert.deepEqual(oversized.playlistIdHashes,[]);
  const empty=await inspectHashRows([],0,0);assert.deepEqual(empty.playlistIdHashes,[]);
});

async function exactDurationFixture(fields) {
  const calls=[],base=fixtureFetch(calls),id='1234567890123456789012';
  return executeJob({provider:'spotify',operation:'getTrack',args:{trackId:id},cookieHeader:SECRET_COOKIE,cachedAuth:{accessToken:'TEST-FIXTURE-CREDENTIAL-6E4DB573E9-cached-access-token-fixture',expiresAt:Date.now()+60000}},{fetchImpl:async(url,options)=>{
    if(new URL(url).pathname===`/v1/tracks/${id}`)return Response.json({id,name:'Preserved metadata',artists:[{name:'Fixture Artist'}],album:{name:'Fixture Album'},...fields});
    return base(url,options);
  }});
}
test('exact Spotify track lookup retains only validated recording-context metadata',async()=>{
 const albumId='ABCDEFGHIJKLMNOPQRSTUV',trackId='1234567890123456789012';
 const result=await exactDurationFixture({album:{name:'Fixture Album',id:albumId,uri:`spotify:album:${albumId}`,release_date:'1988-05-03'},track_number:2,disc_number:1,external_ids:{isrc:'usabc2600001'}});
 assert.equal(result.ok,true,JSON.stringify(result));assert.equal(JSON.stringify(result.result.items[0].data.recordingContext),JSON.stringify({albumId,albumUri:`spotify:album:${albumId}`,trackNumber:2,discNumber:1,isrc:'USABC2600001',releaseDate:'1988-05-03'}));
 const legacy=await exactDurationFixture({album:{name:'Fixture Album'}});assert.equal(legacy.ok,true);assert.equal(Object.hasOwn(legacy.result.items[0].data,'recordingContext'),false);
 const conflicting=await exactDurationFixture({album:{name:'Fixture Album',id:albumId,uri:'spotify:album:ZYXWVUTSRQPONMLKJIHGFE'}});assert.equal(conflicting.ok,false);assert.equal(conflicting.error.code,'PROVIDER_CHANGED');
 const conflictingPosition=await exactDurationFixture({album:{name:'Fixture Album',id:albumId},trackNumber:1,track_number:2});assert.equal(conflictingPosition.ok,false);assert.equal(conflictingPosition.error.code,'PROVIDER_CHANGED');
});

test('duration scalar guard rejects malformed leading fields and preserves fractional fallback',async()=>{
  for(const malformed of [true,[215000],{toString:'not callable'}]){
    const result=await exactDurationFixture({duration:{totalMilliseconds:malformed},duration_ms:245480});
    assert.equal(result.ok,true,JSON.stringify(result));assert.equal(result.result.items[0].data.durationSeconds,245.48);
    assert.equal(result.result.items[0].data.title,'Preserved metadata');
  }
});
test('duration scalar guard leaves malformed-only durations unknown rather than positive coercions',async()=>{
  for(const malformed of [true,[215.5],{toString:'not callable'}]){
    const result=await exactDurationFixture({durationSeconds:malformed});
    assert.equal(result.ok,true,JSON.stringify(result));assert.equal(result.result.items[0].data.durationSeconds,null);
  }
});
test('duration scalar guard retains existing numeric-string inputs and fractional precision',async()=>{
  for(const fields of [{duration_ms:'245480'},{durationSeconds:'245.48'},{duration:{totalMilliseconds:'245480.5'}}]){
    const result=await exactDurationFixture(fields);
    assert.equal(result.ok,true,JSON.stringify(result));assert.equal(result.result.items[0].data.durationSeconds,fields.duration?245.4805:245.48);
  }
});

test('reviewed Spotify UUID intent creates only reconstructed private empty title and verifies fresh readback',async()=>{
 const intent='123e4567-e89b-42d3-a456-426614174000',title=`Burrata verification ${intent}`,calls=[],base=fixtureFetch(calls);let writes=0;
 const fixture={fetchImpl:async(url,options)=>{const u=new URL(url);if(u.hostname==='api.spotify.com'&&u.pathname==='/v1/me/playlists'){writes++;assert.deepEqual(JSON.parse(options.body),{name:title,public:false,collaborative:false});return Response.json({...verificationRaw(),name:title});}if(u.hostname==='api.spotify.com'&&u.pathname===`/v1/playlists/${VERIFICATION_ID}`)return Response.json({...verificationRaw(),name:title});return base(url,options);}};
 const common={provider:'spotify',cookieHeader:SECRET_COOKIE};const args={accountId:'fixture-account',verificationIntentId:intent};
 const created=await executeJob({...common,creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args},fixture);assert.equal(created.ok,true,JSON.stringify(created));assert.equal(created.result.title,title);assert.equal(created.result.creationReceipt.mutationDispatched,true);
 const read=await executeJob({...common,operation:'verifyVerificationPlaylist',args:{...args,playlistId:VERIFICATION_ID}},fixture);assert.equal(read.ok,true,JSON.stringify(read));assert.equal(read.result.playlist.title,title);assert.equal(writes,1);
 for(const bad of [{...args,title:'arbitrary'},{...args,description:'hidden'},{...args,verificationIntentId:'../bad'}]){const result=await executeJob({...common,creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:bad},fixture);assert.equal(result.ok,false);assert.equal(writes,1);}
});
