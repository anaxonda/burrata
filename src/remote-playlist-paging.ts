export type PlaylistPageRow = { id: string };

/** Returns null when committing a whole page would cross the bounded collection. */
export function appendPageWithinLimit<T extends PlaylistPageRow>(current: T[], page: T[], maxRows: number): T[] | null {
  const seen = new Set(current.map(item => item.id));
  const combined = [...current];
  for (const item of page) {
    if (seen.has(item.id)) continue;
    seen.add(item.id);
    combined.push(item);
  }
  return combined.length > maxRows ? null : combined;
}
