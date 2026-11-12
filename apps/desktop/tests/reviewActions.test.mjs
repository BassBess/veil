import { test } from "node:test";
import assert from "node:assert/strict";
import { applyReviewDecision, matchKey, prepareProcessing } from "../src/reviewActions.ts";

function item(source, value, id, category = "PERSON", status = "pending", from = 0) {
  const offset = source.indexOf(value, from);
  assert.notEqual(offset, -1);
  const start = Array.from(source.slice(0, offset)).length;
  return { id, entity_type: category, text: value, start, end: start + Array.from(value).length, status, confidence: .9 };
}
test("one name decision accepts all pending repeats while preserving explicit rejection", () => {
  const source = "John Smith met John Smith and John Smith.";
  const rows = [item(source, "John Smith", "1"), item(source, "John Smith", "2", "PERSON", "pending", 11), item(source, "John Smith", "3", "PERSON", "rejected", 25)];
  const result = applyReviewDecision(source, rows, ["1"], "accepted", true, true);
  assert.deepEqual(result.items.map(x => x.status), ["accepted", "accepted", "rejected"]);
  assert.equal(result.changed, 2);
  assert.equal(result.added, 0);
});
test("accept finds missed full mentions with Unicode offsets and case/whitespace variation", () => {
  const source = "😀 John Smith met JOHN\nSMITH, not John Smithson or XJohn Smith.";
  const result = applyReviewDecision(source, [item(source, "John Smith", "1")], ["1"], "accepted", true, true);
  assert.equal(result.added, 1);
  assert.deepEqual(result.items.map(x => x.text), ["John Smith", "JOHN\nSMITH"]);
  const chars = Array.from(source);
  for (const row of result.items) assert.equal(chars.slice(row.start, row.end).join(""), row.text);
});
test("disabling linked decisions changes one occurrence only", () => {
  const source = "John met John.";
  const rows = [item(source, "John", "1"), item(source, "John", "2", "PERSON", "pending", 5)];
  assert.deepEqual(applyReviewDecision(source, rows, ["1"], "accepted").items.map(x => x.status), ["accepted", "pending"]);
});
test("rejection links pending repeats but preserves previously accepted exceptions", () => {
  const source = "John John John";
  const rows = [item(source, "John", "1"), item(source, "John", "2", "PERSON", "pending", 5), item(source, "John", "3", "PERSON", "accepted", 10)];
  assert.deepEqual(applyReviewDecision(source, rows, ["1"], "rejected", true).items.map(x => x.status), ["rejected", "rejected", "accepted"]);
});
test("category-scoped bulk review does not modify another category with the same text", () => {
  const source = "Paris Paris";
  const rows = [item(source, "Paris", "1", "PERSON"), item(source, "Paris", "2", "LOCATION", "pending", 6)];
  const ids = rows.filter(x => x.entity_type === "PERSON").map(x => x.id);
  assert.deepEqual(applyReviewDecision(source, rows, ids, "accepted").items.map(x => x.status), ["accepted", "pending"]);
});
test("bulk decisions cover matching items beyond the first display page", () => {
  const source = "John ".repeat(110);
  const rows = Array.from({length:110}, (_, i) => ({id:String(i),entity_type:"PERSON",text:"John",start:i*5,end:i*5+4,status:"pending",confidence:.9}));
  const result = applyReviewDecision(source, rows, rows.map(x => x.id), "accepted");
  assert.equal(result.changed, 110);
  assert.equal(prepareProcessing(source, result.items, false).length, 110);
});
test("stale or detached ranges are never bulk-accepted but can be rejected", () => {
  const source = "John";
  const rows = [{...item(source, "John", "1"), text:"Jane"}, {...item(source, "John", "2"),start:4,end:4,needsCorrection:true}];
  const result = applyReviewDecision(source, rows, ["1","2"], "accepted");
  assert.equal(result.skipped, 2);
  assert.ok(result.items.every(x => x.status === "pending"));
  assert.ok(applyReviewDecision(source, rows, ["1","2"], "rejected").items.every(x => x.status === "rejected"));
});
test("overlapping active ranges cannot be accepted together or processed", () => {
  const source = "John Smith";
  const rows = [item(source, "John Smith", "1"), item(source, "John", "2")];
  assert.equal(applyReviewDecision(source, rows, ["1","2"], "accepted").skipped, 2);
  assert.throws(() => prepareProcessing(source, rows, true), /overlap/);
});
test("a repeated match does not overwrite a conflicting category or revive rejection", () => {
  const source = "John Smith / John Smith / John Smith";
  const rows = [item(source, "John Smith", "1"), item(source, "John Smith", "2", "ORGANIZATION", "pending", 11), item(source, "John Smith", "3", "PERSON", "rejected", 24)];
  const result = applyReviewDecision(source, rows, ["1"], "accepted", true, true);
  assert.equal(result.items.length, 3);
  assert.equal(result.items[1].entity_type, "ORGANIZATION");
  assert.equal(result.items[1].status, "pending");
  assert.equal(result.items[2].status, "rejected");
});
test("literal names with regex characters do not broaden the match", () => {
  const source = "A. Smith / Ax Smith / A. Smith";
  const result = applyReviewDecision(source, [item(source, "A. Smith", "1")], ["1"], "accepted", true, true);
  assert.equal(result.items.length, 2);
  assert.ok(result.items.every(x => x.text === "A. Smith"));
});
test("quick processing includes pending and accepted, excluding explicit rejections", () => {
  const source = "John Sarah Alice";
  const rows = [item(source, "John", "1", "PERSON", "accepted"), item(source, "Sarah", "2"), item(source, "Alice", "3", "PERSON", "rejected")];
  assert.deepEqual(prepareProcessing(source, rows, true).map(x => x.id), ["1", "2"]);
  assert.deepEqual(prepareProcessing(source, rows, false).map(x => x.id), ["1"]);
  assert.deepEqual(rows.map(x => x.status), ["accepted", "pending", "rejected"]);
});
test("accepted-only processing does not wait for an unresolved pending correction", () => {
  const source = "John Sarah";
  const rows = [item(source, "John", "1", "PERSON", "accepted"), {...item(source, "Sarah", "2"), needsCorrection:true}];
  assert.equal(prepareProcessing(source, rows, false).length, 1);
  assert.throws(() => prepareProcessing(source, rows, true), /correction/);
});
test("normalization groups names only within their category", () => {
  assert.equal(matchKey({text:" John  Smith ",entity_type:"PERSON"}), matchKey({text:"JOHN SMITH",entity_type:"PERSON"}));
  assert.notEqual(matchKey({text:"John Smith",entity_type:"PERSON"}), matchKey({text:"John Smith",entity_type:"ORGANIZATION"}));
});
