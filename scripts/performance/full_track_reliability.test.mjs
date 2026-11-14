import {test} from 'node:test';
import assert from 'node:assert/strict';
import {summarize} from './full_track_reliability.mjs';
test('failed starts and interrupted songs stay in reliability denominators', () => {
  const report = summarize([
    {started: false, outcome: 'startup_timeout'},
    {started: true, outcome: 'failed', serviceFirstAudioMs: 200},
    {started: true, outcome: 'completed', serviceFirstAudioMs: 100},
  ]);
  assert.equal(report.attempts, 3);
  assert.equal(report.failedStarts, 1);
  assert.equal(report.completionRate, 1 / 3);
  assert.equal(report.startedCompletionRate, 1 / 2);
  assert.deepEqual(report.successfulStartLatencyMs, {p50: 100, p95: 200, p99: 200});
});
