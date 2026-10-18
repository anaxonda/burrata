import test from 'node:test';
import assert from 'node:assert/strict';
import {hasPlaybackAdvanced} from './playback_progress.mjs';

const state = (position, currentIndex = 0) => ({status: 'playing', position, currentIndex});
test('first snapshot or a frozen playing flag cannot qualify advancement', () => {
  assert.equal(hasPlaybackAdvanced(null, state(0)), false);
  assert.equal(hasPlaybackAdvanced(state(0), state(0)), false);
  assert.equal(hasPlaybackAdvanced(state(30), state(30)), false);
});
test('observed movement qualifies; a backwards position on the same occurrence does not', () => {
  assert.equal(hasPlaybackAdvanced(state(0), state(1)), true);
  assert.equal(hasPlaybackAdvanced(state(30), state(2)), false);
});
test('next duplicate occurrence can reset position without appearing frozen', () => {
  assert.equal(hasPlaybackAdvanced(state(359, 0), state(0.1, 1)), true);
  assert.equal(hasPlaybackAdvanced(state(359, 1), state(0.1, 0)), false);
  assert.equal(hasPlaybackAdvanced(state(359, 0), state(0.1, 2)), false);
});
test('unknown positions, occurrence indices and paused state do not qualify', () => {
  assert.equal(hasPlaybackAdvanced(state(0), state(NaN)), false);
  assert.equal(hasPlaybackAdvanced(state(0), state(1, -1)), false);
  assert.equal(hasPlaybackAdvanced(state(0), {...state(1), status: 'paused'}), false);
});
