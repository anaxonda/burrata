import test from 'node:test';
import assert from 'node:assert/strict';
import { acceptedEntryOwnershipChanged, matchesAcceptedEntry, playbackSourceIdentity } from './saved_track_playback_fence.mjs';

test('WebView snapshot without generation matches receipt by occurrence and track', () => {
  const receipt = {
    status: 'accepted',
    engine: 'webview',
    trackIds: ['spotify:track-target'],
    queueEntryIds: ['queued-occurrence-1'],
    acceptedGeneration: 0,
  };
  // Shape published by playback_state; WebView state does not require a generation.
  const webview = {
    engine: 'webview', status: 'playing', trackId: 'spotify:track-target',
    entryId: 'queued-occurrence-1', position: 41, duration: 240,
  };
  assert.equal('generation' in webview, false);
  assert.equal(receipt.acceptedGeneration, 0);
  assert.equal(matchesAcceptedEntry(webview, receipt.trackIds[0], receipt.queueEntryIds[0]), true);
  assert.equal(acceptedEntryOwnershipChanged(webview, receipt.trackIds[0], receipt.queueEntryIds[0]), false);
});

test('old current item is ignored until the accepted occurrence publishes', () => {
  const previous = { engine: 'webview', status: 'paused', trackId: 'spotify:old', entryId: 'old-occurrence' };
  assert.equal(matchesAcceptedEntry(previous, 'spotify:track-target', 'queued-occurrence-1'), false);
  assert.equal(acceptedEntryOwnershipChanged(previous, 'spotify:track-target', 'queued-occurrence-1'), true);
});

test('a different occurrence ends attribution after the accepted entry was observed', () => {
  const next = { engine: 'webview', status: 'playing', trackId: 'spotify:track-target', entryId: 'later-occurrence' };
  assert.equal(matchesAcceptedEntry(next, 'spotify:track-target', 'queued-occurrence-1'), false);
  assert.equal(acceptedEntryOwnershipChanged(next, 'spotify:track-target', 'queued-occurrence-1'), true);
});

test('WebView lastResolvedSource is used when candidate is absent, while native candidate wins', () => {
  assert.deepEqual(playbackSourceIdentity({
    engine: 'webview',
    lastResolvedSource: { id: 'YtExample_123', provider: 'youtube', headerPolicy: 'opaque', sourceTransport: 'webview' },
  }), { provider: 'youtube', id: 'YtExample_123' });
  assert.deepEqual(playbackSourceIdentity({
    engine: 'native',
    candidate: { id: 'bandcamp-recording-1', provider: 'bandcamp' },
    lastResolvedSource: { id: 'stale', provider: 'youtube' },
  }), { provider: 'bandcamp', id: 'bandcamp-recording-1' });
  assert.deepEqual(playbackSourceIdentity({
    lastResolvedSource: { id: 'unsafe?query=secret', provider: 'youtube' },
  }), { provider: 'youtube', id: null });
});
