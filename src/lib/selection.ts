/** Invert selection within a scope while preserving selected items outside it. */
export function invertSelection(scopeKeys: readonly string[], selectedKeys: readonly string[]): string[] {
  const scope = [...new Set(scopeKeys)];
  const selected = new Set(selectedKeys);
  const inScope = new Set(scope);
  const inverted = scope.filter(key => !selected.has(key));
  const outside = [...new Set(selectedKeys)].filter(key => !inScope.has(key));
  return [...inverted, ...outside];
}
