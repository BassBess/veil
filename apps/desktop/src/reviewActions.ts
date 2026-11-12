export type ReviewItem = {
  id: string; entity_type: string; text: string; start: number; end: number;
  status: "pending" | "accepted" | "rejected"; confidence: number | null; needsCorrection?: boolean;
};
type Decision = ReviewItem["status"];
export function groupDetections<T extends ReviewItem>(items: T[]): T[][] {
  const groups = new Map<string, T[]>();
  for (const item of items) {
    const key = matchKey(item);
    const group = groups.get(key);
    if (group) group.push(item); else groups.set(key, [item]);
  }
  return [...groups.values()];
}
const foldedCategories = new Set(["PERSON", "ORGANIZATION", "LOCATION", "STREET_ADDRESS", "INDIRECT_IDENTIFIER"]);

export function matchKey(item: Pick<ReviewItem, "entity_type" | "text">): string {
  const value = foldedCategories.has(item.entity_type)
    ? item.text.trim().replace(/\s+/gu, " ").toLowerCase() : item.text.trim();
  return JSON.stringify([item.entity_type, value]);
}
function currentRange(chars: string[], item: ReviewItem): boolean {
  return !item.needsCorrection && Number.isInteger(item.start) && Number.isInteger(item.end)
    && item.start >= 0 && item.start < item.end && item.end <= chars.length
    && chars.slice(item.start, item.end).join("") === item.text;
}
function lowerBound(starts: number[], value: number): number {
  let left = 0, right = starts.length;
  while (left < right) {
    const middle = (left + right) >>> 1;
    if (starts[middle] < value) left = middle + 1; else right = middle;
  }
  return left;
}
function conflictIds(items: ReviewItem[]): Set<string> {
  const conflicts = new Set<string>();
  let previous: ReviewItem | undefined;
  for (const item of [...items].sort((a, b) => a.start - b.start)) {
    if (previous && previous.end > item.start) {
      conflicts.add(previous.id); conflicts.add(item.id);
    }
    if (!previous || item.end > previous.end) previous = item;
  }
  return conflicts;
}

// Single-item decisions link pending repeats only. Explicit bulk decisions act
// on the requested IDs. Neither path silently revives a rejected repeat.
export function applyReviewDecision(
  source: string, items: ReviewItem[], ids: string[], status: Decision,
  linkRepeated = false, addMissing = false,
): { items: ReviewItem[]; changed: number; added: number; skipped: number } {
  const chars = Array.from(source);
  const selected = new Set(ids);
  const targets = items.filter(item => selected.has(item.id));
  const keys = new Set(targets.filter(item => currentRange(chars, item)).map(matchKey));
  if (linkRepeated && status !== "pending") {
    for (const item of items) {
      if (item.status === "pending" && keys.has(matchKey(item))) selected.add(item.id);
    }
  }
  const invalid = new Set(items.filter(item => selected.has(item.id) && !currentRange(chars, item)).map(item => item.id));
  const active = items.filter(item => currentRange(chars, item)
    && (item.status !== "rejected" || selected.has(item.id)));
  const conflicts = status === "accepted" ? conflictIds(active) : new Set<string>();
  let changed = 0, skipped = 0, added = 0;
  const next = items.map(item => {
    if (!selected.has(item.id)) return item;
    if (status === "accepted" && (invalid.has(item.id) || conflicts.has(item.id))) {
      skipped++; return item;
    }
    if (item.status !== status) changed++;
    return { ...item, status };
  });
  if (status !== "accepted" || !linkRepeated || !addMissing) return { items: next, changed, added, skipped };

  // Index original ranges, including explicit rejections, so inferred mentions
  // cannot overwrite another category or a per-occurrence exception.
  const occupied = items.filter(item => item.end > item.start).sort((a, b) => a.start - b.start);
  const starts = occupied.map(item => item.start);
  let furthest = -1;
  const maxEnds = occupied.map(item => (furthest = Math.max(furthest, item.end)));
  const additions: ReviewItem[] = [];
  const additionStarts: number[] = [];
  const offsets = new Map<number, number>();
  let units = 0, points = 0;
  offsets.set(0, 0);
  for (const char of source) { units += char.length; points++; offsets.set(units, points); }
  const seen = new Set<string>();
  for (const target of targets) {
    const key = matchKey(target);
    if (seen.has(key) || !next.some(item => item.id === target.id && item.status === "accepted")) continue;
    seen.add(key);
    const escape = (value: string) => value.replace(/[.*+?^$()|[\]{}\\]/g, "\\$&");
    const folded = foldedCategories.has(target.entity_type);
    const pattern = folded ? target.text.trim().split(/\s+/u).map(escape).join("\\s+") : escape(target.text.trim());
    if (!pattern) continue;
    const expression = new RegExp(pattern, folded ? "giu" : "gu");
    const word = (char: string | undefined) => !!char && /[\p{L}\p{N}_]/u.test(char);
    for (const match of source.matchAll(expression)) {
      const start = offsets.get(match.index), end = offsets.get(match.index + match[0].length);
      if (start === undefined || end === undefined) continue;
      if ((word(chars[start]) && word(chars[start - 1])) || (word(chars[end - 1]) && word(chars[end]))) continue;
      const last = lowerBound(starts, end) - 1;
      if (last >= 0 && maxEnds[last] > start) continue;
      const position = lowerBound(additionStarts, start);
      if ((position > 0 && additions[position - 1].end > start)
        || (position < additions.length && additions[position].start < end)) continue;
      const item: ReviewItem = {
        id: crypto.randomUUID(), entity_type: target.entity_type,
        text: match[0], start, end, status: "accepted", confidence: null,
      };
      additions.splice(position, 0, item); additionStarts.splice(position, 0, start);
      added++; changed++;
    }
  }
  return { items: [...next, ...additions].sort((a, b) => a.start - b.start), changed, added, skipped };
}

export function prepareProcessing(source: string, items: ReviewItem[], includePending: boolean): ReviewItem[] {
  const chars = Array.from(source);
  const selected = items.filter(item => item.status === "accepted" || (includePending && item.status === "pending"));
  if (!selected.length) throw new Error("No detections are available for this action.");
  if (selected.some(item => !currentRange(chars, item))) {
    throw new Error("Some detections need correction. Edit or reject those items before processing.");
  }
  if (conflictIds(selected).size) {
    throw new Error("Some selected detections overlap. Correct or reject the conflicting items.");
  }
  return selected.map(item => ({ ...item, status: "accepted" }));
}
