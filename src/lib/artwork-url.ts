/** Accept only small, credential-free HTTPS artwork URLs from normalized provider DTOs. */
export function safeArtworkUrl(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0 || value.length > 2048) return null;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || !url.hostname || url.username || url.password) return null;
    if (url.port && url.port !== "443") return null;
    return value;
  } catch {
    return null;
  }
}

export function safeArtworkUrls(values: readonly unknown[], limit = 8): string[] {
  const cap = Math.max(0, Math.min(8, Math.floor(limit)));
  if (cap === 0) return [];
  const seen = new Set<string>();
  const urls: string[] = [];
  for (const value of values) {
    const url = safeArtworkUrl(value);
    if (!url || seen.has(url)) continue;
    seen.add(url);
    urls.push(url);
    if (urls.length >= cap) break;
  }
  return urls;
}
