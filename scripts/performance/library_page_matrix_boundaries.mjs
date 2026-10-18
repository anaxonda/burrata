export function additionalPageBoundaryOffsets(total, pageSize = 200, verifiedOffsets = [0, pageSize]) {
  if (!Number.isSafeInteger(total) || total < 0 || !Number.isSafeInteger(pageSize) || pageSize < 1) {
    throw new TypeError('total and pageSize must be nonnegative/positive safe integers');
  }
  const verified = new Set(verifiedOffsets);
  if (total <= pageSize) return [];
  const middle = Math.floor((total / 2) / pageSize) * pageSize;
  const tail = Math.floor((total - 1) / pageSize) * pageSize;
  return [...new Set([middle, tail])]
    .filter(offset => offset > 0 && offset < total && !verified.has(offset))
    .sort((a, b) => a - b);
}
