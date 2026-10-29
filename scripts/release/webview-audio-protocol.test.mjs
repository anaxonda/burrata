import test from 'node:test';
import assert from 'node:assert/strict';
import { waitForFrontendReady, webviewMediaProbeConfirmsEngine } from './webview-audio-protocol.mjs';

test('accepts the actual WebView DTO when playback_state omits engine', () => {
  const playbackState = { status: 'idle', trackId: null, queueTotal: 0 };
  const mediaInspection = { engine: 'webview', applicable: true, media: { present: false } };
  assert.equal(webviewMediaProbeConfirmsEngine(playbackState, mediaInspection), true);
});

test('requires the media inspection itself to confirm applicable WebView ownership', () => {
  const playbackState = { status: 'idle' };
  assert.equal(webviewMediaProbeConfirmsEngine(playbackState,
    { engine: 'native', applicable: false }), false);
  assert.equal(webviewMediaProbeConfirmsEngine(playbackState,
    { engine: 'webview', applicable: false }), false);
  assert.equal(webviewMediaProbeConfirmsEngine({ engine: 'native' },
    { engine: 'webview', applicable: true }), false);
});

test('waits only for the explicit frontend-not-ready response before the first media sample', async () => {
  let now = 0;
  let attempts = 0;
  const media = { engine: 'webview', applicable: true };
  const result = await waitForFrontendReady(async () => {
    attempts += 1;
    if (attempts < 3) throw Object.assign(new Error('not ready'), { code: 'FRONTEND_NOT_READY' });
    return media;
  }, {
    timeoutMs: 1_000,
    pollIntervalMs: 250,
    now: () => now,
    sleep: async ms => { now += ms; },
  });
  assert.equal(result, media);
  assert.equal(attempts, 3);
  assert.equal(now, 500);
});

test('does not retry terminal inspection errors', async () => {
  let attempts = 0;
  const failure = Object.assign(new Error('inspection failed'), { code: 'TIMEOUT' });
  await assert.rejects(waitForFrontendReady(async () => {
    attempts += 1;
    throw failure;
  }, { timeoutMs: 1_000 }), error => error === failure);
  assert.equal(attempts, 1);
});

test('frontend readiness deadline expires only after explicit not-ready replies', async () => {
  let now = 0;
  let attempts = 0;
  await assert.rejects(waitForFrontendReady(async () => {
    attempts += 1;
    throw Object.assign(new Error('not ready'), { code: 'FRONTEND_NOT_READY' });
  }, {
    timeoutMs: 500,
    pollIntervalMs: 250,
    now: () => now,
    sleep: async ms => { now += ms; },
  }), error => error.code === 'FRONTEND_NOT_READY');
  assert.equal(attempts, 2);
  assert.equal(now, 500);
});

import { createWebviewAudioSmokeGate } from './webview-audio-protocol.mjs';

const trackId = 'local:synthetic-fixture';
function ready(currentTime, overrides = {}) {
  return { engine:'webview', applicable:true, trackId, queueTrackId:trackId,
    entryId:'fixture-entry', queueTotal:1, sourceGeneration:3, sessionId:'fixture-session', seekSerial:0, previewKind:'local',
    media:{present:true, sourceStable:true, paused:false, readyState:4, networkState:1,
      duration:4, currentTime, errorCode:null, seeking:false, ended:false, buffered:[[0,4]], seekable:[[0,4]]}, ...overrides };
}
const playback = {status:'playing', trackId};
const hasCategory = category => error => error.category === category;

test('startup preroll does not become the steady baseline until the same clock advances', () => {
  const gate = createWebviewAudioSmokeGate(trackId, 0);
  // Actual failed-run samples: readiness was complete while the clock was still starting.
  assert.equal(gate.observe(playback, ready(0.013834), 744.6), 'startup');
  assert.equal(gate.observe(playback, ready(0.116099774), 1650.9), 'startup');
  assert.equal(gate.observe(playback, ready(0.66), 2200), 'steady');
  assert.equal(gate.observe(playback, ready(1.56), 3100), 'steady');
  assert.equal(gate.observe(playback, ready(2.46), 4000), 'steady');
  assert.equal(gate.observe(playback, ready(3.36), 4900), 'passed');
  const evidence = gate.diagnostics(4910);
  assert.equal(evidence.initialReady.media.currentTime, 0.013834);
  assert.equal(evidence.firstProgress.media.currentTime, 0.66);
  assert.equal(evidence.first.media.currentTime, 0.66);
  assert.equal(evidence.sampling.initialReadyElapsedMs, 744.6);
  assert.equal(evidence.sampling.firstProgressElapsedMs, 2200);
  assert.equal(evidence.sampling.betweenSamplesWallMs, 2700);
  assert.equal(evidence.sampling.validatedSteadyWindows, 3);
});

test('one successful window cannot hide a stall in a later steady window', () => {
  const gate = createWebviewAudioSmokeGate(trackId, 0);
  gate.observe(playback, ready(0), 0);
  gate.observe(playback, ready(0.6), 1000);
  assert.equal(gate.observe(playback, ready(1.5), 1900), 'steady');
  assert.throws(() => gate.observe(playback, ready(1.6), 2800), hasCategory('steady_clock_stalled'));
  assert.equal(gate.diagnostics(2800).sampling.validatedSteadyWindows, 1);
});

test('a permanently stalled ready source fails the original startup deadline', () => {
  const gate = createWebviewAudioSmokeGate(trackId, 0);
  gate.observe(playback, ready(0.013834), 0);
  gate.observe(playback, ready(0.013834), 9900);
  assert.throws(() => gate.expireStartup(), hasCategory('audio_clock_not_started'));
  assert.equal(gate.phase, 'failed');
  assert.equal(gate.diagnostics(10_000).firstProgress, null);
  assert.throws(() => gate.observe(playback, ready(2), 10_001), /already terminal/);
  assert.equal(gate.diagnostics(10_001).latest.media.currentTime, 0.013834);
});

test('clock progress arriving at the startup deadline cannot reset or extend it', () => {
  const gate = createWebviewAudioSmokeGate(trackId, 0);
  gate.observe(playback, ready(0.01), 0);
  assert.throws(() => gate.observe(playback, ready(0.8), 10_000), hasCategory('audio_clock_not_started'));
  assert.equal(gate.diagnostics(10_000).firstProgress, null);
});

test('continued slow playback fails the unchanged steady clock threshold after startup proof', () => {
  const gate = createWebviewAudioSmokeGate(trackId, 0);
  gate.observe(playback, ready(0.01), 0);
  assert.equal(gate.observe(playback, ready(0.61), 5400), 'steady');
  assert.throws(() => gate.observe(playback, ready(0.7123), 6306), hasCategory('steady_clock_stalled'));
  assert.equal(gate.phase, 'failed');
  assert.equal(gate.diagnostics(6306).sampling.betweenSamplesWallMs, 906);
  assert.throws(() => gate.observe(playback, ready(2), 7206), /already terminal/);
  assert.equal(gate.diagnostics(7206).second.media.currentTime, 0.7123);
});

test('superseded entry, session, or source generation cannot satisfy startup or steady progress', () => {
  for (const change of [{entryId:'other-entry'}, {sessionId:'other-session'}, {sourceGeneration:4}]) {
    for (const steady of [false, true]) {
      const gate = createWebviewAudioSmokeGate(trackId, 0);
      gate.observe(playback, ready(0.01), 0);
      if (steady) gate.observe(playback, ready(0.61), 1000);
      assert.throws(() => gate.observe(playback, ready(1.5, change), 1900), hasCategory('playback_identity_changed'));
      assert.equal(gate.phase, 'failed');
    }
  }
});
