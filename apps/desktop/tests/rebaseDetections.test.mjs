import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rebaseDetections } from '../src/rebaseDetections.ts';

const entity = (text, start, end, status = 'accepted') => ({ id: 'kept-id', entity_type: 'PERSON', text, start, end, status, confidence: .9 });

test('preserves review decisions and categories while shifting Unicode positions', () => {
  const item = entity('John', 0, 4);
  const [result] = rebaseDetections('John lives here', '😀 John lives here', [item]);
  assert.deepEqual(result, { ...item, start: 2, end: 6 });
});
test('edits outside the detection preserve accepted and rejected decisions', () => {
  for (const status of ['accepted', 'rejected']) {
    const item = entity('John', 0, 4, status);
    assert.deepEqual(rebaseDetections('John lives here', 'John lives there', [item]), [item]);
  }
});
test('replacement inside a detection updates text and resets review', () => {
  const [result] = rebaseDetections('John Smith', 'Jane Smith', [entity('John Smith', 0, 10)]);
  assert.equal(result.text, 'Jane Smith');
  assert.equal(result.status, 'pending');
  assert.equal(result.confidence, null);
  assert.equal(result.id, 'kept-id');
});
test('deletion retains the item for correction or rejection instead of losing review history', () => {
  const [result] = rebaseDetections('John lives here', ' lives here', [entity('John', 0, 4)]);
  assert.equal(result.text, 'John');
  assert.equal(result.needsCorrection, true);
  assert.equal(result.status, 'pending');
  assert.equal(result.start, result.end);
  const [stillDetached] = rebaseDetections(' lives here', 'X lives here', [result]);
  assert.equal(stillDetached.needsCorrection, true);
  assert.equal(stillDetached.start, stillDetached.end);
});
test('insertions at range boundaries shift or preserve without resetting review', () => {
  const item = entity('John', 0, 4);
  assert.equal(rebaseDetections('John', 'John!', [item])[0].status, 'accepted');
  assert.deepEqual(rebaseDetections('John', '!John', [item])[0], { ...item, start: 1, end: 5 });
});
test('insertion within a range updates its text and length', () => {
  const [result] = rebaseDetections('John', 'Jo😀hn', [entity('John', 0, 4)]);
  assert.equal(result.text, 'Jo😀hn');
  assert.equal(result.end, 5);
  assert.equal(result.status, 'pending');
});
