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
