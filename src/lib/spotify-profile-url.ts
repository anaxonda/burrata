/** Validate and canonicalize an official Spotify legacy user-profile URL. */
export function normalizeSpotifyUserProfileUrl(value: string): string | null {
  try {
    const parsed = new URL(value);
    if (parsed.protocol !== "https:" || parsed.hostname !== "open.spotify.com"
      || parsed.username || parsed.password || (parsed.port && parsed.port !== "443")
      || /[?#]/.test(value)) return null;
    const match = parsed.pathname.match(/^\/user\/([^/]+)\/?$/);
    if (!match) return null;
    const id = decodeURIComponent(match[1]!);
    if (!id || id.length > 180 || /[\s\u0000-\u001f\u007f/\\?#]/u.test(id)) return null;
    return `https://open.spotify.com/user/${encodeURIComponent(id)}`;
  } catch { return null; }
}
