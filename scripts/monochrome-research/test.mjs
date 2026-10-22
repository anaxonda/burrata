import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('./capture.js', import.meta.url), 'utf8');
const pure = vm.createContext({ URL });
vm.runInContext(source, pure);
const { sanitize, route, isResolver, requestMetadata } = pure.BurrataMonochromeResearchTest;
const fixture = {
  schema_version: '2.0', track: { title: 'Alvafro', artists: [{ name: 'LATIN MAFIA' }], isrc: 'GBAHS2600508' },
  access_token: 'private-token', authorization: 'Bearer fixture',
  playback: [{ kind: 'audio', delivery: 'direct', codec: 'flac', bit_depth: 16, sample_rate_hz: 44100,
    url: 'https://cdn.test/song.mp4?signature=private', expires_at: '2026-10-21T20:00:00Z',
    encryption: { scheme: 'cenc', key: { value: 'private-media', id: 'private-kid', encoding: 'BASE64' }, key_id: 'kid', keys: [{ id: 'another-kid', value: 'more-private' }], iv: 'private-iv', iv_source: 'manifest' },
  }],
};
test('retains useful response structure and metadata, redacts every key/token leaf', () => {
  const value = sanitize(fixture);
  assert.equal(value.track.title, 'Alvafro');
  assert.equal(value.playback[0].delivery, 'direct');
  assert.equal(value.playback[0].encryption.scheme, 'cenc');
  assert.equal(value.playback[0].encryption.key.encoding, 'base64');
  assert.equal(value.playback[0].encryption.iv_source, 'manifest');
  assert.equal(value.playback[0].encryption.key.value, '[REDACTED]');
  assert.equal(value.playback[0].expires_at, '2026-10-21T20:00:00Z');
  assert.equal(value.playback[0].encryption.key.id, '[REDACTED]');
  assert.equal(value.playback[0].encryption.keys[0].id, '[REDACTED]');
  assert.equal(value.playback[0].url, '[REDACTED]');
  assert.doesNotMatch(JSON.stringify(value), /private|another-kid|Bearer fixture/);
});
test('redacts URLs and credential-shaped strings even under metadata keys; bounds input', () => {
  assert.equal(sanitize({ title: 'https://cdn.test/?key=secret' }).title, '[REDACTED]');
  assert.equal(sanitize({ name: 'a'.repeat(64) }).name, '[REDACTED]');
  assert.equal(sanitize({ list: Array(100).fill(1) }).list.length, 40);
  let nested = { title: 'End' }; for (let n = 0; n < 20; n++) nested = { nested };
  assert.match(JSON.stringify(sanitize(nested)), /DEPTH LIMIT/);
  const broad = sanitize({ rows: Array(40).fill({ fields: Array(40).fill({ fields: Array(40).fill({ title: 'kept?' }) }) }) });
  assert.match(JSON.stringify(broad), /NODE LIMIT/);
});
test('endpoint inventory drops all queries, fragments, credentials and opaque path segments', () => {
  assert.equal(route('https://user:password@api.test/api/v2/track/?key=private#token'), 'https://api.test/api/v2/track/');
  assert.equal(route('https://api.test/0123456789abcdef0123456789abcdef.js'), 'https://api.test/[REDACTED].js');
  assert.equal(route('data:text/plain,private'), null);
  assert.equal(route(undefined), null);
});
test('ignores unrelated account JSON, recognizes old and new resolver envelopes', () => {
  assert.equal(!!isResolver({ account: { token: 'private' } }), false);
  assert.equal(!!isResolver(fixture), true);
  assert.equal(!!isResolver({ data: { manifest: 'opaque' } }), true);
});
test('request metadata keeps allowlisted query values and header presence only', () => {
  const metadata = requestMetadata('https://api.test/resolve?track=trk-123&artist=Artist&album=Record&isrc=USABC1234567&duration=245&intent=play&quality=flac&token=private&debug=secret', 'post',
    { Authorization: 'Bearer private-access-token', 'X-Turnstile-JWT': 'private-jwt', 'Content-Type': 'application/json', Cookie: 'private-cookie' });
  assert.deepEqual(JSON.parse(JSON.stringify(metadata)), {
    method: 'POST',
    query: { track: 'trk-123', artist: 'Artist', album: 'Record', isrc: 'USABC1234567', duration: '245', intent: 'play', quality: 'flac' },
    headers: { authorizationBearer: true, turnstileJwtPresent: true, contentTypePresent: true },
  });
  assert.doesNotMatch(JSON.stringify(metadata), /private-access-token|private-jwt|private-cookie|token=|debug=|Bearer\s/i);
  const unsafe = requestMetadata('https://api.test/resolve?track=super-secret-token&artist=https%3A%2F%2Fsecret.test&quality=flac', 'GET');
  assert.deepEqual(JSON.parse(JSON.stringify(unsafe.query)), { quality: 'flac' });
});
function page(origin = 'https://monochrome.samidy.com') {
  class Xhr { open() {} send() {} addEventListener() {} removeEventListener() {} }
  let next = Promise.resolve(new Response(JSON.stringify(fixture), { headers: { 'content-type': 'application/json' } }));
  const original = () => next;
  const c = vm.createContext({ URL, Response, Uint8Array, TextDecoder, WeakMap, Set, Date, XMLHttpRequest: Xhr,
    location: { origin }, navigator: {}, performance: { getEntriesByType: () => [{ initiatorType: 'fetch', name: 'https://api.test/api/v2/track?token=private' }] },
    fetch: original, setTimeout() {} });
  c.window = c; c.top = c;
  vm.runInContext(source, c);
  return { c, original, next, replace: value => { next = value; } };
}
test('fetch observer returns the original promise/response and restores wrappers on stop', async () => {
  const { c, original, next } = page();
  assert.equal(c.fetch('https://api.test/api/v2/track?private'), next);
  await next; await new Promise(resolve => setTimeout(resolve, 30));
  const report = c.__burrataMonochromeResearchV1.read();
  assert.equal(report.responses.length, 1);
  assert.doesNotMatch(JSON.stringify(report), /private|token=/);
  c.__burrataMonochromeResearchV1.stop();
  assert.equal(c.fetch, original);
  assert.equal(c.__burrataMonochromeResearchV1.read().stopped, true);
});
test('fetch capture reports sanitized request metadata without retaining headers or arbitrary query keys', async () => {
  const { c, next } = page();
  const request = { url: 'https://api.test/resolve?track=trk-123&quality=flac&token=private-token&debug=secret', method: 'POST',
    headers: { Authorization: 'Bearer private-access-token', 'X-Turnstile-JWT': 'private-jwt' } };
  await c.fetch(request, { method: 'POST', headers: { 'Content-Type': 'application/json' } });
  await next; await new Promise(resolve => setTimeout(resolve, 20));
  const report = c.__burrataMonochromeResearchV1.read();
  assert.deepEqual(JSON.parse(JSON.stringify(report.responses[0].request)), {
    method: 'POST', query: { track: 'trk-123', quality: 'flac' },
    headers: { authorizationBearer: true, turnstileJwtPresent: true, contentTypePresent: true },
  });
  assert.doesNotMatch(JSON.stringify(report), /private-access-token|private-jwt|private-token|debug=|Bearer\s/i);
});
test('capture refuses other origins and skips oversized JSON without consuming original body', async () => {
  const other = page('https://unrelated.test');
  assert.equal(other.c.fetch, other.original);
  assert.equal(other.c.__burrataMonochromeResearchV1, undefined);
  const { c, replace } = page();
  const response = new Response(JSON.stringify(fixture), { headers: { 'content-type': 'application/json', 'content-length': '999999' } });
  replace(Promise.resolve(response));
  await c.fetch('https://api.test/track'); await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(c.__burrataMonochromeResearchV1.read().responses.length, 0);
  assert.equal(response.bodyUsed, false);
});

test('XHR observer captures bounded sanitized JSON responses and restores its hooks', () => {
  const listeners = new Map();
  class Xhr {
    open() {}
    send() {}
    setRequestHeader() {}
    addEventListener(name, fn) { listeners.set(name, fn); }
    removeEventListener(name, fn) { if (listeners.get(name) === fn) listeners.delete(name); }
    getResponseHeader() { return 'application/json'; }
  }
  const originalOpen = Xhr.prototype.open;
  const originalSend = Xhr.prototype.send;
  const originalSetRequestHeader = Xhr.prototype.setRequestHeader;
  const c = vm.createContext({ URL, Response, Uint8Array, TextDecoder, WeakMap, Set, Date, XMLHttpRequest: Xhr,
    location: { origin: 'https://monochrome.samidy.com' }, navigator: {}, performance: { getEntriesByType: () => [] },
    fetch: () => Promise.resolve(new Response('{}')), setTimeout() {} });
  c.window = c; c.top = c;
  vm.runInContext(source, c);
  const xhr = new Xhr();
  xhr.open('GET', 'https://api.test/v2/track?token=private');
  xhr.setRequestHeader('Authorization', 'Bearer xhr-private-access');
  xhr.setRequestHeader('X-Turnstile-JWT', 'xhr-private-jwt');
  xhr.responseType = 'json';
  xhr.response = fixture;
  xhr.status = 200;
  xhr.send();
  listeners.get('load')();
  const report = c.__burrataMonochromeResearchV1.read();
  assert.equal(report.responses.length, 1);
  assert.equal(report.responses[0].response.track.title, 'Alvafro');
  assert.equal(report.responses[0].request.headers.authorizationBearer, true);
  assert.equal(report.responses[0].request.headers.turnstileJwtPresent, true);
  assert.doesNotMatch(JSON.stringify(report), /private|token=/);
  c.__burrataMonochromeResearchV1.stop();
  assert.equal(Xhr.prototype.open, originalOpen);
  assert.equal(Xhr.prototype.send, originalSend);
  assert.equal(Xhr.prototype.setRequestHeader, originalSetRequestHeader);
});
