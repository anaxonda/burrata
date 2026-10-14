import type { Candidate, Track } from "./types";

function tokens(value: string): string[] {
  return value.toLowerCase().normalize("NFKD").replace(/[\u0300-\u036f]/g, "")
    .replace(/[^\p{L}\p{N}]+/gu, " ").trim().split(/\s+/).filter(Boolean);
}

function coverage(expected: string[], actual: string[]): number {
  if (!expected.length) return 0;
  const found = new Set(actual);
  return expected.filter((word) => found.has(word)).length / expected.length;
}

export function plausiblePreviewCandidate(track: Track, candidate: Candidate): boolean {
  if (!(["youtube", "soundcloud", "monochrome"].includes(candidate.provider)) || !candidate.url ||
      !Number.isFinite(candidate.score) || candidate.score < 0.55 || candidate.confidence === "low") {
    return false;
  }
  if (candidate.provider === "monochrome") return candidate.confidence === "high" && validMonochromeUrl(candidate);
  const title = tokens(track.title), artist = tokens(track.artist);
  const candidateTitle = tokens(candidate.title);
  const candidateArtist = tokens(`${candidate.title} ${candidate.uploader ?? ""}`);
  return coverage(title, candidateTitle) >= 0.6 && coverage(artist, candidateArtist) >= 0.5;
}

/** A search result can use its exact linked source without catalog-identity scoring. */
export function isLinkedAudioSearchCandidate(track: Track, candidate: Candidate): boolean {
  const provenance = track.provenance && typeof track.provenance === "object"
    ? track.provenance as Record<string, unknown> : {};
  if (!["audioSearch", "youtubeRadio"].includes(String(provenance.sourceKind)) ||
      !["youtube", "soundcloud", "monochrome"].includes(candidate.provider) ||
      typeof candidate.id !== "string" || !candidate.id.trim() ||
      typeof candidate.title !== "string" || !candidate.title.trim() ||
      typeof candidate.url !== "string") return false;
  try {
    const url = new URL(candidate.url);
    if (url.protocol !== "https:" || url.username || url.password) return false;
    const host = url.hostname.toLowerCase();
    if (candidate.provider === "monochrome") return validMonochromeUrl(candidate) && track.provider === "monochrome" && candidate.id === track.id;
    const providerHost = candidate.provider === "youtube"
      ? host === "youtube.com" || host.endsWith(".youtube.com") || host === "youtu.be"
      : host === "soundcloud.com" || host.endsWith(".soundcloud.com");
    return providerHost && url.pathname !== "/";
  } catch {
    return false;
  }
}

/** Prefer a plausible YouTube result; SoundCloud is a fallback provider. */
export function choosePreviewCandidate(
  track: Track,
  candidates: Candidate[],
  provider: "youtube" | "soundcloud" | "monochrome",
  excluded: Set<string> = new Set(),
): Candidate | undefined {
  return candidates
    .filter((candidate) => candidate.provider === provider && !excluded.has(`${candidate.provider}:${candidate.id}`) && plausiblePreviewCandidate(track, candidate))
    .sort((a, b) => b.score - a.score)[0];
}

function validMonochromeUrl(candidate: Candidate): boolean {
  try {
    const url = new URL(candidate.url);
    return url.protocol === "https:" && url.hostname === "tracks.monochrome.st" && !url.port && !url.username && !url.password && !url.search && !url.hash && /^\/track\/\d{17,20}$/.test(url.pathname);
  } catch { return false; }
}

/** The playlist URL stays authoritative even after remembering a different played source. */
export function playlistUrlCandidate(track: Track, remembered?: Candidate): Candidate | undefined {
  const provenance = track.provenance as Record<string, unknown> | undefined;
  if (track.provider !== "import" || provenance?.sourceKind !== "audioSearch") return remembered;
  const original = typeof provenance.playlistEntry === "string" ? provenance.playlistEntry : track.sourceUrl;
  if (!original) return remembered;
  if (remembered?.url === original) return remembered;
  try {
    const url = new URL(original);
    const host = url.hostname.toLowerCase();
    const provider = host === "youtube.com" || host.endsWith(".youtube.com") || host === "youtu.be" ? "youtube"
      : host === "soundcloud.com" || host.endsWith(".soundcloud.com") ? "soundcloud" : undefined;
    if (!provider) return undefined;
    const candidate: Candidate = { id: original, provider, url: original, title: track.title,
      uploader: null, durationSeconds: null, artworkUrl: track.artworkUrl ?? null, formats: [],
      confidence: "low", score: 0, permission: "unknown", official: false,
      reasons: ["Explicit playlist URL; recording identity is unverified"] };
    return isLinkedAudioSearchCandidate(track, candidate) ? candidate : undefined;
  } catch { return undefined; }
}
