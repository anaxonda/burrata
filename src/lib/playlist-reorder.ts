/** Convert a drop before/after a 1-based playlist occurrence into the final 0-based index. */
export function playlistDropIndex(fromPosition: number, targetPosition: number, after: boolean): number | null {
  if (!Number.isInteger(fromPosition) || fromPosition < 1 || !Number.isInteger(targetPosition) || targetPosition < 1) return null;
  if (fromPosition === targetPosition) return null;
  const from = fromPosition - 1;
  const target = targetPosition - 1;
  const insertionPoint = target + (after ? 1 : 0);
  return insertionPoint > from ? insertionPoint - 1 : insertionPoint;
}
