import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Exercise the controller's actual gate without an app, provider or audio server.
const source = readFileSync(new URL('./native_runtime_capture.mjs', import.meta.url), 'utf8');
const branch = source.split("} else if(mode==='full-nipsey'){")[1].split("} else if(mode==='full-incidents'){")[0];
const expression = branch.match(/report\.passed=([\s\S]*?);\n    await call\('pause_playback'\)/)[1];
const evaluate = new Function('report', 'finished', 'stable', 'once', 'monotonic', 'receiptError', `return (${expression});`);
const qualified = { matches: true, ownerStable: true, metadataUnchanged: true };
function passes(recognition, failureEvidence = { lastSkippedTrackFailure: null, recentTrackFailures: [] }) {
  return evaluate({ cases: [{ passed: true }], recognition, failureEvidence }, true, true, true, true, null);
}

test('full Nipsey qualification requires affirmative recognition with stable ownership and metadata', () => {
  assert.equal(passes(qualified), true);
  for (const absent of [undefined, null, {}, { error: 'recognition unavailable' }]) assert.equal(passes(absent), false);
  for (const field of Object.keys(qualified)) {
    for (const value of [false, null, undefined]) assert.equal(passes({ ...qualified, [field]: value }), false, `${field}: ${value}`);
  }
});

test('recognition cannot hide a skipped or failed recording', () => {
  assert.equal(passes(qualified, { lastSkippedTrackFailure: { entryId: 'failed' }, recentTrackFailures: [] }), false);
  assert.equal(passes(qualified, { lastSkippedTrackFailure: null, recentTrackFailures: [{ entryId: 'failed' }] }), false);
});
