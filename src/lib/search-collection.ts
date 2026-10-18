export type CollectionRow = { id: string; sourceId: string; kind: string; track?: unknown | null };

/** Takes a bounded number of provider results, regardless of row kind. */
export function takeCollectionRows<T>(rows: T[], remaining: number): { accepted: T[]; overflow: T[] } {
  const count = Math.max(0, Math.floor(remaining));
  return { accepted: rows.slice(0, count), overflow: rows.slice(count) };
}

/** Splits a provider page without losing rows when a collection reaches its per-action cap. */
export function splitCollectionRows<T extends CollectionRow>(existingRows: T[], incomingRows: T[], trackCap: number): { accepted: T[]; overflow: T[] } {
  const identity = (row: CollectionRow) => `${row.sourceId}:${row.kind}:${row.id}`;
  const known = new Set(existingRows.map(identity));
  let remaining = Math.max(0, trackCap - existingRows.filter(row => Boolean(row.track)).length);
  const accepted: T[] = [];
  const overflow: T[] = [];
  for (const row of incomingRows) {
    const key = identity(row);
    if (known.has(key)) { accepted.push(row); continue; }
    if (!row.track) { known.add(key); accepted.push(row); continue; }
    if (remaining <= 0) { overflow.push(row); continue; }
    known.add(key);
    remaining--;
    accepted.push(row);
  }
  return { accepted, overflow };
}
