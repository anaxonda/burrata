/** Clamp a virtual table window when filtering shrinks a previously scrolled list. */
export function rowWindow(
  count: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight = 60,
  overscan = 8,
  virtualizationThreshold = 1000,
): { start: number; end: number } {
  if (count <= virtualizationThreshold) return { start: 0, end: count };
  const visible = Math.max(1, Math.ceil(viewportHeight / rowHeight));
  const start = Math.max(
    0,
    Math.min(count - visible, Math.floor(scrollTop / rowHeight) - overscan),
  );
  return { start, end: Math.min(count, start + visible + overscan * 2) };
}
