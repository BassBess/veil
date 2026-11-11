type ReviewableRange = {
  start: number;
  end: number;
  text: string;
  status: "pending" | "accepted" | "rejected";
  confidence: number | null;
  needsCorrection?: boolean;
};

// Treat the changed middle as a replacement, using Python-compatible code points.
// Unchanged prefixes/suffixes retain review decisions; ambiguous ranges need review.
export function rebaseDetections<T extends ReviewableRange>(
  previousText: string,
  nextText: string,
  detections: T[],
): T[] {
  if (previousText === nextText) return detections;
  const before = Array.from(previousText);
  const after = Array.from(nextText);
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let oldEnd = before.length;
  let newEnd = after.length;
  while (oldEnd > start && newEnd > start && before[oldEnd - 1] === after[newEnd - 1]) {
    oldEnd--;
    newEnd--;
  }
  const delta = newEnd - oldEnd;
  return detections.map((item) => {
    if (item.end <= start && !item.needsCorrection) return item;
    if (item.start >= oldEnd && !item.needsCorrection) {
      return { ...item, start: item.start + delta, end: item.end + delta };
    }
    // Detached items remain visible until explicitly corrected or rejected.
    if (item.needsCorrection) {
      const anchor = Math.min(item.start <= start ? item.start : item.start >= oldEnd ? item.start + delta : start, after.length);
      return { ...item, start: anchor, end: anchor };
    }
    const nextStart = item.start < start ? item.start : start;
    const nextEnd = item.end > oldEnd ? item.end + delta : newEnd;
    const selectedText = after.slice(nextStart, nextEnd).join("");
    return {
      ...item,
      start: nextStart,
      end: nextEnd,
      text: selectedText.trim() ? selectedText : item.text,
      status: "pending",
      confidence: null,
      needsCorrection: !selectedText.trim(),
    };
  });
}
