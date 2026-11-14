import {test} from 'node:test';
import assert from 'node:assert/strict';
import {completionOutcome, summarize} from './full_track_reliability.mjs';
test('clean EOF must cover the requested duration to qualify as completion', () => {
  const result = {started: true, expectedDurationSeconds: 226.232};
  assert.equal(completionOutcome({naturalQueueEnd: true, position: 15}, result), 'early_end');
  assert.equal(completionOutcome({naturalQueueEnd: false, position: 226.195}, result), 'unverified_end');
  assert.equal(completionOutcome({naturalQueueEnd: true, position: 226.195}, result), 'completed');
  assert.equal(completionOutcome({naturalQueueEnd: true, position: 226.195}, {started: true}), 'missing_expected_duration');
});
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
