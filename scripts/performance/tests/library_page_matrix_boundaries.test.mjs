import { describe, it } from 'node:test';
import { strict as assert } from 'node:assert';
import { additionalPageBoundaryOffsets } from '../library_page_matrix_boundaries.mjs';

describe('additionalPageBoundaryOffsets', () => {
  it('skips pages already covered for short and exactly two-page totals', () => {
    assert.deepEqual(additionalPageBoundaryOffsets(80), []);
    assert.deepEqual(additionalPageBoundaryOffsets(400), []);
  });

  it('samples middle and tail pages once without affecting first/continuation checks', () => {
    assert.deepEqual(additionalPageBoundaryOffsets(500), [400]);
    assert.deepEqual(additionalPageBoundaryOffsets(1000), [400, 800]);
    assert.deepEqual(additionalPageBoundaryOffsets(1000, 200, [0, 200, 400]), [800]);
  });

  it('rejects invalid pagination dimensions', () => {
    assert.throws(() => additionalPageBoundaryOffsets(-1), /safe integers/);
    assert.throws(() => additionalPageBoundaryOffsets(10, 0), /safe integers/);
  });
});
