import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { buildVisionRequest, buildVisitorRequests, CONSTRUCTION_VERSION, networkCause, parseAnonymousVisitor,
  playabilityCategory, PROFILE, runVisionosProbe, selectVisionAudio, VISITOR_CONSTRUCTION_VERSION } from './visionos_probe.mjs';

const VIDEO = 'uZGRATckoBM';
const request = extra => ({ videoId: VIDEO, expectedVideoId: VIDEO, hl: 'en', gl: 'US', budgetMs: 1000, ...extra });
function body() {
  return {
    playabilityStatus: { status: 'OK' },
    videoDetails: { videoId: VIDEO, title: 'fixture title must not be reported', author: 'fixture author must not be reported', lengthSeconds: '260' },
    streamingData: { expiresInSeconds: '3600', adaptiveFormats: [{ itag: 251,
      mimeType: 'audio/webm; codecs="opus"', contentLength: '4000000', approxDurationMs: '260020',
      url: `https://r1---fixture.googlevideo.com/videoplayback?expire=${Math.floor(Date.now() / 1000) + 3600}&sig=private-signature` }] },
  };
}
const okAudio = () => ({ firstPcmMs: 3, decodedFrames: 240000, decodedAudioMs: 5000,
  decodeWallMs: 5, decoderCpuMs: 2, decoderMaxRssBytes: 1024 });
const response = data => new Response(JSON.stringify(data), { status: 200, headers: { 'content-type': 'application/json' } });
const visitorGolden = JSON.parse(readFileSync(new URL('./fixtures/visionos-01-anonymous-visitor-source.json', import.meta.url), 'utf8'));
function serviceWorkerBody(value = visitorGolden.syntheticVisitor) {
  const row = Array(14).fill(null); row[13] = value;
  return `)]}'\n${JSON.stringify([[null, null, [[row]]]])}`;
}

test('request matches an independently source-derived golden with canonical excerpts and source hashes', () => {
  const golden = JSON.parse(readFileSync(new URL('./fixtures/visionos-01-source-request.json', import.meta.url), 'utf8'));
  assert.equal(golden.constructionVersion, CONSTRUCTION_VERSION);
  const spec = buildVisionRequest(golden.request);
  assert.deepEqual({ endpoint: spec.endpoint, headers: spec.headers, body: spec.body }, golden.expected);
  assert.equal(golden.provenance.length, 5);
  assert.ok(golden.provenance.every(source => /^[a-f0-9]{64}$/.test(source.sha256) && source.sourceExcerpt.length));
});

test('request exactly uses pinned 0.1 music profile without token, player, visitor, auth or media headers', () => {
  const spec = buildVisionRequest(request());
  assert.equal(spec.endpoint, 'https://music.youtube.com/youtubei/v1/player?prettyPrint=false');
  assert.equal(spec.headers['X-YouTube-Client-Name'], '101');
  assert.equal(spec.headers.Accept, 'application/json');
  assert.equal(spec.headers['Cache-Control'], 'no-cache');
  assert.equal(spec.headers['X-YouTube-Client-Version'], '0.1');
  assert.equal(spec.headers['User-Agent'], PROFILE.userAgent);
  assert.equal(spec.headers.Origin, 'https://music.youtube.com');
  assert.equal(spec.headers['X-Origin'], 'https://music.youtube.com');
  assert.equal(spec.headers.Referer, 'https://music.youtube.com/');
  assert.equal(spec.headers['Accept-Language'], 'en-US,en;q=0.9');
  assert.deepEqual(spec.body, {
    context: { client: { clientName: 'VISIONOS', clientVersion: '0.1', osName: 'VISION_OS', osVersion: '1.3',
      deviceMake: 'Apple', deviceModel: 'RealityDevice14,1', platform: 'MOBILE', gl: 'US', hl: 'en' },
    request: { internalExperimentFlags: [], useSsl: true }, user: { lockedSafetyMode: false } },
    videoId: VIDEO, contentCheckOk: true, racyCheckOk: true,
  });
  assert.equal(spec.headers.Cookie, undefined);
  assert.equal(spec.headers.Authorization, undefined);
  assert.equal(spec.body.playbackContext, undefined);
  assert.equal(spec.body.serviceIntegrityDimensions, undefined);
  assert.equal(spec.body.videoCheckOk, undefined);
  assert.equal(buildVisionRequest(request({ hl: 'fr_CA', gl: 'CA' })).headers['Accept-Language'], 'fr-CA,fr;q=0.9');
});

test('anonymous request cannot accept account fields, invented locale or unbounded deadline', () => {
  for (const extra of [{ cookie: 'private' }, { visitorData: 'private' }, { hl: '' }, { gl: 'us' },
    { expectedVideoId: 'AAAAAAAAAAA' }, { formatId: -1 }, { budgetMs: 20001 }]) {
    assert.throws(() => buildVisionRequest(request(extra)));
  }
});

test('exact ID, fixed itag and five seconds decode return only allowlisted metrics', async () => {
  let fetched = 0, decoded = 0;
  const result = await runVisionosProbe(request({ id: 'exact-fixture' }), {
    async fetchImpl(url, init) {
      fetched++;
      assert.equal(url, PROFILE.endpoint);
      assert.equal(init.redirect, 'error');
      assert.equal(init.credentials, 'omit');
      assert.equal(JSON.parse(init.body).context.client.clientVersion, '0.1');
      return response(body());
    },
    async decode(url, options) {
      decoded++;
      assert.equal(new URL(url).searchParams.get('cpn').length, 16);
      assert.deepEqual(options.headers, {});
      assert.equal(options.seconds, 5);
      return { ...okAudio(), unexpectedSecret: 'private-decoder-value' };
    },
  });
  assert.equal(fetched, 1);
  assert.equal(decoded, 1);
  assert.equal(result.ok, true);
  assert.equal(result.profileId, 'VISIONOS_0_1');
  assert.equal(result.formatId, 251);
  assert.equal(result.returnedVideoId, VIDEO);
  assert.equal(result.metadata.durationSeconds, 260);
  assert.equal(result.qualification, false);
  assert.deepEqual(result.mediaHeaderNames, []);
  const serialized = JSON.stringify(result);
  for (const secret of ['private-signature', 'googlevideo.com', 'fixture title', 'fixture author', 'private-decoder-value']) {
    assert.equal(serialized.includes(secret), false, secret);
  }
});

test('wrong or missing upstream ID fails before CDN/decode despite matching requested IDs', async () => {
  for (const actual of ['AAAAAAAAAAA', undefined]) {
    const data = body();
    data.videoDetails.videoId = actual;
    let decoded = false;
    const result = await runVisionosProbe(request(), { fetchImpl: async () => response(data),
      decode: async () => { decoded = true; return okAudio(); } });
    assert.equal(result.failure, 'RECORDING_MISMATCH');
    assert.equal(decoded, false);
  }
});

test('optional corpus metadata is compared internally and reported only as a match boolean', async () => {
  const expectedMetadata = { titleNeedle: 'fixture title', channelNeedle: 'fixture author', durationSeconds: 260 };
  const matched = await runVisionosProbe(request({ expectedMetadata }), {
    fetchImpl: async () => response(body()), decode: async () => okAudio(),
  });
  assert.equal(matched.ok, true);
  assert.equal(matched.metadata.matchesExpectedMetadata, true);
  for (const extra of [{ titleNeedle: 'another recording' }, { channelNeedle: 'another artist' }, { durationSeconds: 200 }]) {
    const result = await runVisionosProbe(request({ expectedMetadata: { ...expectedMetadata, ...extra } }), {
      fetchImpl: async () => response(body()), decode: async () => assert.fail('mismatch must not decode'),
    });
    assert.equal(result.failure, 'METADATA_MISMATCH');
  }
  assert.throws(() => buildVisionRequest(request({ expectedMetadata: { ...expectedMetadata, cookie: 'private' } })));
});

test('fixed-format arm reports available formats without silently switching itag', async () => {
  const data = body();
  data.streamingData.adaptiveFormats[0].itag = 140;
  const result = await runVisionosProbe(request(), { fetchImpl: async () => response(data),
    decode: async () => assert.fail('strict itag mismatch must not decode') });
  assert.equal(result.failure, 'FORMAT_UNAVAILABLE');
  assert.equal(result.availableFormats[0].formatId, 140);
  assert.equal(result.formatId, undefined);
});

test('cipher and n-transform requirements fail rather than invoking player JavaScript', () => {
  for (const field of ['signatureCipher', 'cipher']) {
    const data = body();
    data.streamingData.adaptiveFormats[0][field] = 'private-cipher';
    assert.throws(() => selectVisionAudio(data, request()), error => error.code === 'CIPHER_REQUIRED');
  }
  for (const query of ['&n=private-n', '&%6e=private-n', '%26n%3Dprivate-n']) {
    const data = body();
    data.streamingData.adaptiveFormats[0].url += query;
    assert.throws(() => selectVisionAudio(data, request()), error => error.code === 'N_TRANSFORM_REQUIRED');
  }
});

test('status, preview, length, expiry, and untrusted transport guards precede decoding', () => {
  const cases = [
    ['PLAYABILITY_REJECTED', data => { data.playabilityStatus.status = 'LOGIN_REQUIRED'; }],
    ['PREVIEW_OR_TRUNCATED_FORMAT', data => { data.streamingData.adaptiveFormats[0].approxDurationMs = '30000'; }],
    ['FORMAT_LENGTH_MISSING', data => { delete data.streamingData.adaptiveFormats[0].contentLength; }],
    ['FORMAT_DURATION_MISSING', data => { delete data.streamingData.adaptiveFormats[0].approxDurationMs; }],
    ['MANIFEST_EXPIRED', data => { data.streamingData.adaptiveFormats[0].url = 'https://example.googlevideo.com/videoplayback?expire=1'; }],
    ['INVALID_MEDIA_URL', data => { data.streamingData.adaptiveFormats[0].url = 'http://127.0.0.1/videoplayback'; }],
    ['UNSUPPORTED_TRANSPORT', data => { delete data.streamingData.adaptiveFormats[0].url; data.streamingData.hlsManifestUrl = 'private-manifest'; }],
  ];
  for (const [code, mutate] of cases) {
    const data = body(); mutate(data);
    assert.throws(() => selectVisionAudio(data, request()), error => error.code === code, code);
  }
});

test('playability reason classification emits fixed conservative categories without raw text', async () => {
  assert.equal(playabilityCategory({ playabilityStatus: { status: 'UNPLAYABLE' } }), 'UNCLASSIFIED_REJECTION');
  assert.equal(playabilityCategory({ playabilityStatus: { status: 'LOGIN_REQUIRED' } }), 'LOGIN_REQUIRED');
  assert.equal(playabilityCategory({ playabilityStatus: { status: 'UNPLAYABLE', reason: 'Confirm your age' } }), 'AGE_RESTRICTED');
  const data = body();
  data.playabilityStatus = { status: 'UNPLAYABLE', reason: 'This video is not available. https://private.example/?token=secret' };
  const result = await runVisionosProbe(request(), { fetchImpl: async () => response(data) });
  assert.equal(result.playabilityCategory, 'UNAVAILABLE');
  assert.equal(result.failure, 'PLAYABILITY_REJECTED');
  assert.equal(JSON.stringify(result).includes('private.example'), false);
  assert.equal(JSON.stringify(result).includes('secret'), false);
});

test('caller cancellation aborts the pending player request and preserves only a fixed category', async () => {
  const controller = new AbortController();
  let fetchSignal;
  const pending = runVisionosProbe(request(), { signal: controller.signal,
    fetchImpl: async (_url, init) => { fetchSignal = init.signal; return new Promise(() => {}); } });
  await new Promise(resolve => setImmediate(resolve));
  controller.abort(Object.assign(new Error('private-url-must-not-escape'), { code: 'CANCELLED' }));
  const result = await pending;
  assert.equal(fetchSignal.aborted, true);
  assert.equal(result.failure, 'CANCELLED');
  assert.equal(JSON.stringify(result).includes('private-url'), false);
});

test('caller cancellation reaches a pending decoder without exposing its transport', async () => {
  const controller = new AbortController();
  let decodeSignal;
  const pending = runVisionosProbe(request(), { signal: controller.signal,
    fetchImpl: async () => response(body()),
    decode: async (_url, options) => { decodeSignal = options.signal; return new Promise(() => {}); },
  });
  while (!decodeSignal) await new Promise(resolve => setImmediate(resolve));
  controller.abort(Object.assign(new Error('private-decoder-url'), { code: 'CANCELLED' }));
  const result = await pending;
  assert.equal(decodeSignal.aborted, true);
  assert.equal(result.failure, 'CANCELLED');
  assert.equal(JSON.stringify(result).includes('private-decoder-url'), false);
});

test('one absolute deadline bounds pending fetch and oversized response is rejected', async () => {
  const timeout = await runVisionosProbe(request({ budgetMs: 20 }), { fetchImpl: async () => new Promise(() => {}) });
  assert.equal(timeout.failure, 'TIMEOUT');
  const large = await runVisionosProbe(request(), { fetchImpl: async () => new Response('{}', {
    status: 200, headers: { 'content-length': String(4 * 1024 * 1024 + 1) },
  }) });
  assert.equal(large.failure, 'RESPONSE_TOO_LARGE');
});

test('five-second result rejects short PCM and never exposes request/decoder errors', async () => {
  const short = await runVisionosProbe(request(), { fetchImpl: async () => response(body()),
    decode: async () => ({ ...okAudio(), decodedFrames: 1000 }) });
  assert.equal(short.failure, 'SHORT_AUDIO');
  const failed = await runVisionosProbe(request(), { fetchImpl: async () => { throw new Error('https://private.example/?token=secret'); } });
  assert.equal(failed.failure, 'NETWORK_FAILED');
  assert.equal(JSON.stringify(failed).includes('secret'), false);
});

test('visitor lookup requests match source-derived golden and bind exactly one anonymous value', () => {
  assert.deepEqual(buildVisitorRequests(visitorGolden.request), visitorGolden.expectedLookups);
  assert.ok(visitorGolden.source.sourceExcerpt.includes('jsonArray[13]'));
  const spec = buildVisionRequest(visitorGolden.request, { visitorData: visitorGolden.syntheticVisitor });
  assert.equal(spec.headers['X-Goog-Visitor-Id'], visitorGolden.syntheticVisitor);
  assert.equal(spec.body.context.client.visitorData, visitorGolden.syntheticVisitor);
  for (const header of ['Cookie', 'Authorization', 'X-Goog-AuthUser']) assert.equal(spec.headers[header], undefined);
  for (const extra of [{ visitorData: 'cannot-inject-session' }, { visitorMode: 'account' }]) {
    assert.throws(() => buildVisionRequest(request(extra)));
  }
  assert.throws(() => buildVisionRequest(request(), { visitorData: visitorGolden.syntheticVisitor }));
});

test('source visitor parsers accept the exact service-worker location or homepage key, never another ID', () => {
  assert.equal(parseAnonymousVisitor(serviceWorkerBody(), 'service-worker'), visitorGolden.syntheticVisitor);
  assert.equal(parseAnonymousVisitor(serviceWorkerBody().split('\n')[1], 'service-worker'), visitorGolden.syntheticVisitor);
  for (const key of ['VISITOR_DATA', 'visitorData']) {
    assert.equal(parseAnonymousVisitor(`<script>var x={"${key}":"${visitorGolden.syntheticVisitor}"}</script>`, 'music-homepage'), visitorGolden.syntheticVisitor);
  }
  for (const value of [null, 123, '', 'line\r\nbreak', ' ', 'x'.repeat(4097)]) {
    assert.equal(parseAnonymousVisitor(serviceWorkerBody(value), 'service-worker'), null);
  }
  assert.equal(parseAnonymousVisitor('{"visitorData":"wrong-shape"}', 'service-worker'), null);
  assert.equal(parseAnonymousVisitor('{"otherId":"wrong-key"}', 'music-homepage'), null);
});

test('explicit anonymous visitor mode performs one GET then one bound player POST and five-second decode', async () => {
  const calls = [];
  const result = await runVisionosProbe({ ...visitorGolden.request, budgetMs: 1000 }, {
    fetchImpl: async (url, init) => {
      calls.push({ url, init });
      assert.equal(init.credentials, 'omit'); assert.equal(init.redirect, 'error');
      assert.equal(init.headers.Cookie, undefined); assert.equal(init.headers.Authorization, undefined);
      if (calls.length === 1) {
        assert.equal(init.method, 'GET');
        return new Response(serviceWorkerBody());
      }
      assert.equal(init.method, 'POST');
      assert.equal(init.headers['X-Goog-Visitor-Id'], visitorGolden.syntheticVisitor);
      assert.equal(JSON.parse(init.body).context.client.visitorData, visitorGolden.syntheticVisitor);
      return response(body());
    }, decode: async (_url, options) => { assert.equal(options.seconds, 5); return okAudio(); },
  });
  assert.equal(calls.length, 2);
  assert.equal(result.ok, true);
  assert.equal(result.constructionVersion, VISITOR_CONSTRUCTION_VERSION);
  assert.equal(result.visitorContext, 'anonymous-source-service-worker; ephemeral-no-session-cache');
  assert.equal(result.visitorAttempts[0].found, true);
  assert.equal(result.requestHeaderNames.includes('X-Goog-Visitor-Id'), true);
  assert.equal(JSON.stringify(result).includes(visitorGolden.syntheticVisitor), false);
  assert.equal(JSON.stringify(result).includes('/sw.js_data'), false);
});

test('source homepage fallback is explicit after missing service-worker data and uses the same binding', async () => {
  const calls = [];
  const result = await runVisionosProbe(request({ visitorMode: 'anonymous-source' }), {
    fetchImpl: async (url, init) => {
      calls.push(url);
      if (calls.length === 1) return new Response('{}');
      if (calls.length === 2) {
        assert.equal(init.method, 'GET');
        assert.equal(init.headers.Accept, visitorGolden.expectedLookups[1].headers.Accept);
        return new Response(`{"VISITOR_DATA":"${visitorGolden.syntheticVisitor}"}`);
      }
      assert.equal(init.headers['X-Goog-Visitor-Id'], visitorGolden.syntheticVisitor);
      assert.equal(JSON.parse(init.body).context.client.visitorData, visitorGolden.syntheticVisitor);
      return response(body());
    }, decode: async () => okAudio(),
  });
  assert.equal(calls.length, 3);
  assert.equal(result.ok, true);
  assert.equal(result.visitorContext, 'anonymous-source-music-homepage; ephemeral-no-session-cache');
  assert.deepEqual(result.visitorAttempts.map(row => row.found), [false, true]);
  assert.equal(JSON.stringify(result).includes(visitorGolden.syntheticVisitor), false);
});

test('unusable anonymous context makes at most two bounded GETs and never sends an unbound player request', async () => {
  let calls = 0;
  const result = await runVisionosProbe(request({ visitorMode: 'anonymous-source' }), {
    fetchImpl: async (_url, init) => { calls++; assert.equal(init.method, 'GET'); return new Response('private server reason', { status: 403 }); },
    decode: async () => assert.fail('no context must not decode'),
  });
  assert.equal(calls, 2);
  assert.equal(result.failure, 'VISITOR_CONTEXT_UNAVAILABLE');
  assert.deepEqual(result.visitorAttempts.map(row => row.httpStatus), [403, 403]);
  assert.equal(JSON.stringify(result).includes('private server reason'), false);
});

test('caller cancellation during visitor lookup aborts its signal and prevents homepage and player work', async () => {
  const controller = new AbortController();
  let calls = 0, lookupSignal;
  const pending = runVisionosProbe(request({ visitorMode: 'anonymous-source' }), {
    signal: controller.signal,
    fetchImpl: async (_url, init) => { calls++; lookupSignal = init.signal; return new Promise(() => {}); },
    decode: async () => assert.fail('cancelled context must not decode'),
  });
  await new Promise(resolve => setImmediate(resolve));
  controller.abort(Object.assign(new Error('private-context-value'), { code: 'CANCELLED' }));
  const result = await pending;
  assert.equal(result.failure, 'CANCELLED');
  assert.equal(lookupSignal.aborted, true);
  assert.equal(calls, 1);
  assert.equal(JSON.stringify(result).includes('private-context-value'), false);
});

test('context and player share one absolute budget rather than receiving separate deadlines', async () => {
  let calls = 0;
  const result = await runVisionosProbe(request({ visitorMode: 'anonymous-source', budgetMs: 30 }), {
    fetchImpl: async (_url, init) => {
      calls++;
      if (init.method === 'GET') { await new Promise(resolve => setTimeout(resolve, 15)); return new Response(serviceWorkerBody()); }
      return new Promise(() => {});
    }, decode: async () => assert.fail('expired player must not decode'),
  });
  assert.equal(result.failure, 'TIMEOUT');
  assert.ok(calls <= 2);
  assert.equal(result.visitorAttempts.length, 1);
  assert.ok(result.stages.visitorServiceWorker >= 0);
});

test('each context response is capped and oversized bodies cannot reach the player', async () => {
  let calls = 0;
  const result = await runVisionosProbe(request({ visitorMode: 'anonymous-source' }), {
    fetchImpl: async (_url, init) => {
      calls++; assert.equal(init.method, 'GET');
      return new Response('{}', { headers: { 'content-length': String(4 * 1024 * 1024 + 1) } });
    }, decode: async () => assert.fail('oversized context must not decode'),
  });
  assert.equal(calls, 2);
  assert.equal(result.failure, 'VISITOR_CONTEXT_UNAVAILABLE');
  assert.deepEqual(result.visitorAttempts.map(row => row.failure), ['RESPONSE_TOO_LARGE', 'RESPONSE_TOO_LARGE']);
});

test('visitor context does not relax recording, format, status, or cipher protections', async () => {
  for (const [expected, mutate] of [
    ['RECORDING_MISMATCH', data => { data.videoDetails.videoId = 'AAAAAAAAAAA'; }],
    ['FORMAT_UNAVAILABLE', data => { data.streamingData.adaptiveFormats[0].itag = 140; }],
    ['PLAYABILITY_REJECTED', data => { data.playabilityStatus.status = 'UNPLAYABLE'; }],
    ['CIPHER_REQUIRED', data => { data.streamingData.adaptiveFormats[0].signatureCipher = 'private-cipher'; }],
  ]) {
    const data = body(); mutate(data);
    const result = await runVisionosProbe(request({ visitorMode: 'anonymous-source' }), {
      fetchImpl: async (_url, init) => init.method === 'GET' ? new Response(serviceWorkerBody()) : response(data),
      decode: async () => assert.fail('visitor mode must retain strict guards'),
    });
    assert.equal(result.failure, expected);
  }
});

test('diagnostic seek fractions derive from returned duration and reject an out-of-bounds excerpt', async () => {
  for (const fraction of [0.01, 0.5, 0.95]) {
    const result = await runVisionosProbe(request({ positionFraction: fraction }), {
      fetchImpl: async () => response(body()),
      decode: async (_url, options) => {
        assert.equal(options.position, 260 * fraction); assert.equal(options.seconds, 5);
        assert.equal(options.minimumDecodedSeconds, undefined); return okAudio();
      },
    });
    assert.equal(result.ok, true); assert.equal(result.positionSeconds, 260 * fraction);
    assert.equal(result.fullRecording, false);
  }
  const bad = await runVisionosProbe(request({ position: 259 }), {
    fetchImpl: async () => response(body()), decode: async () => assert.fail('out-of-range must not decode'),
  });
  assert.equal(bad.failure, 'INVALID_DECODE_RANGE');
  for (const extra of [{ position: -1 }, { positionFraction: 1 }, { position: 0, positionFraction: 0.5 },
    { seconds: 601 }, { minimumDecodedSeconds: 0 }, { fullRecording: 'yes' },
    { fullRecording: true }, { fullRecording: true, seconds: 5 },
    { fullRecording: true, position: 1 }]) assert.throws(() => buildVisionRequest(request(extra)));
});

test('explicit full recording counts actual PCM within metadata plus or minus one second', async () => {
  const expectedMetadata = { titleNeedle: 'fixture title', channelNeedle: 'fixture author', durationSeconds: 260 };
  const full = request({ fullRecording: true, expectedMetadata });
  const result = await runVisionosProbe(full, {
    fetchImpl: async () => response(body()),
    decode: async (_url, options) => {
      assert.equal(options.position, 0); assert.equal(options.seconds, 261);
      assert.equal(options.minimumDecodedSeconds, 259);
      return { ...okAudio(), decodedFrames: 260 * 48000 - 5000 };
    },
  });
  assert.equal(result.ok, true); assert.equal(result.fullRecordingComplete, true);
  assert.equal(result.expectedDecodeSeconds, 260); assert.equal(result.decoderLimitSeconds, 261);
  assert.equal(result.decodedAudioMs, (260 * 48000 - 5000) / 48);
  for (const [frames, category] of [[60 * 48000, 'TRUNCATED_RECORDING'], [259 * 48000 - 1, 'TRUNCATED_RECORDING'],
    [261 * 48000 + 1, 'FULL_RECORDING_DURATION_MISMATCH']]) {
    const failed = await runVisionosProbe(full, { fetchImpl: async () => response(body()),
      decode: async () => ({ ...okAudio(), decodedFrames: frames }) });
    assert.equal(failed.failure, category);
  }
  const early = await runVisionosProbe(full, { fetchImpl: async () => response(body()),
    decode: async () => { throw Object.assign(new Error('private transport'), { code: 'SHORT_AUDIO' }); } });
  assert.equal(early.failure, 'TRUNCATED_RECORDING');
  assert.equal(JSON.stringify(early).includes('private transport'), false);
});

test('full-recording source duration cap and preview manifest are enforced before decode', async () => {
  const expectedMetadata = { titleNeedle: 'fixture title', channelNeedle: 'fixture author', durationSeconds: 600 };
  const data = body(); data.videoDetails.lengthSeconds = '601'; data.streamingData.adaptiveFormats[0].approxDurationMs = '601000';
  const result = await runVisionosProbe(request({ fullRecording: true, expectedMetadata }), {
    fetchImpl: async () => response(data), decode: async () => assert.fail('over600s cannot decode'),
  });
  assert.equal(result.failure, 'FULL_RECORDING_TOO_LONG');
  const preview = body(); preview.streamingData.adaptiveFormats[0].approxDurationMs = '60000';
  const short = await runVisionosProbe(request({ fullRecording: true,
    expectedMetadata: { ...expectedMetadata, durationSeconds: 260 } }), {
    fetchImpl: async () => response(preview), decode: async () => assert.fail('60-second preview cannot decode'),
  });
  assert.equal(short.failure, 'PREVIEW_OR_TRUNCATED_FORMAT');
});

test('network causes are fixed code-only categories, including nested socket errors, never raw diagnostics', async () => {
  for (const code of ['ECONNRESET', 'UND_ERR_SOCKET', 'ETIMEDOUT', 'ENOTFOUND', 'EAI_AGAIN']) {
    const error = new TypeError('https://private.example/?secret=token', {
      cause: Object.assign(new Error('/private/socket/path'), { code }),
    });
    const result = await runVisionosProbe(request(), { fetchImpl: async () => { throw error; } });
    assert.equal(result.failure, 'NETWORK_FAILED'); assert.equal(result.networkCause, code);
    assert.equal(result.stage, 'player');
    for (const secret of ['private.example', 'secret', '/private/socket']) assert.equal(JSON.stringify(result).includes(secret), false);
  }
  const cycle = { code: 'PRIVATE_UNKNOWN_CODE', message: 'ECONNRESET https://private.example' }; cycle.cause = cycle;
  assert.equal(networkCause(cycle), 'OTHER');
  assert.equal(networkCause(new AggregateError([{ code: 'EAI_AGAIN' }], 'private details')), 'EAI_AGAIN');
  assert.equal(networkCause({ code: 'https://private.example', cause: new Error('ETIMEDOUT') }), 'OTHER');
});
