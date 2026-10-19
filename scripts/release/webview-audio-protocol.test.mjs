import test from 'node:test';
import assert from 'node:assert/strict';
import { webviewMediaProbeConfirmsEngine } from './webview-audio-protocol.mjs';

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
  assert.equal(gate.observe(playback, ready(1.56), 3100), 'passed');
  const evidence = gate.diagnostics(3110);
  assert.equal(evidence.initialReady.media.currentTime, 0.013834);
  assert.equal(evidence.firstProgress.media.currentTime, 0.66);
  assert.equal(evidence.first.media.currentTime, 0.66);
  assert.equal(evidence.sampling.initialReadyElapsedMs, 744.6);
  assert.equal(evidence.sampling.firstProgressElapsedMs, 2200);
  assert.equal(evidence.sampling.betweenSamplesWallMs, 900);
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
