import type { Track } from "./types";

export function trackSource(track: Track): { url: string; label: string } | null {
  const provenance = track.provenance && typeof track.provenance === "object"
    ? track.provenance as Record<string, unknown> : {};
  for (const raw of [track.sourceUrl, provenance.sourceUrl]) {
    if (typeof raw !== "string") continue;
    try {
      const url = new URL(raw);
      if (!["http:", "https:"].includes(url.protocol) || url.username || url.password ||
          !url.hostname.includes(".") || /^(localhost|127\.|0\.|10\.|192\.168\.|169\.254\.)/.test(url.hostname)) continue;
      const host = url.hostname.toLowerCase();
      const label = host === "beatport.com" || host.endsWith(".beatport.com") ? "Beatport"
        : host === "traxsource.com" || host.endsWith(".traxsource.com") ? "Traxsource"
        : host === "spotify.com" || host.endsWith(".spotify.com") ? "Spotify"
        : host === "youtube.com" || host.endsWith(".youtube.com") || host === "youtu.be" ? "YouTube"
        : host === "soundcloud.com" || host.endsWith(".soundcloud.com") ? "SoundCloud" : "source";
      return { url: url.href, label };
    } catch { /* Local metadata can have no web source. */ }
  }
  return null;
}
