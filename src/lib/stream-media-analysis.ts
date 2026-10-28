import { streamWaveformSnapshot, type WaveformBin, type WaveformSnapshot } from "./waveform-data";
import type { Track } from "./types";

export type StreamAudioVariant = {
  formatId?: string | null; codec?: string | null; mimeType?: string | null;
  bitrateKbps?: number | null; expectedContentLength?: number | null; containsVideo?: boolean | null;
};
export type StreamMediaResource = { trackId: string; provider: string; sourceId: string; durationSeconds: number; audioVariant?: StreamAudioVariant | null; sessionGeneration?: number | null };

/** Stable, URL-free encoding identity. Keep this serialization in sync with
 * stream_analysis::StreamAudioVariant; signed URLs and rounded bitrates are
 * deliberately excluded. Unknown encodings are session-only. */
export function streamAudioVariantIdentity(variant: StreamAudioVariant | null | undefined): string | null {
  const token = (value: unknown, max: number, pattern: RegExp): string | null => {
    if (typeof value !== "string") return null;
    const trimmed = value.trim();
    // Rust uses ASCII case folding. Reject Unicode characters that JavaScript
    // could fold into ASCII (for example the Kelvin sign) before hashing.
    if (!/^[\x00-\x7f]*$/.test(trimmed)) return null;
    const normalized = trimmed.toLowerCase();
    return normalized.length > 0 && normalized.length <= max && pattern.test(normalized) ? normalized : null;
  };
  const format = token(variant?.formatId, 96, /^[a-z0-9._+-]+$/);
  const codec = token(variant?.codec, 96, /^[a-z0-9._+-]+$/);
  const mime = token(variant?.mimeType, 128, /^[a-z0-9/._+-]+$/);
  const validMime = mime?.includes("/") ? mime : null;
  if (!format || (!codec && !validMime)) return null;
  const length = variant?.expectedContentLength;
  if (length != null && (typeof length !== "number" || !Number.isSafeInteger(length) || length <= 0)) return null;
  const lengthText = typeof length === "number" && Number.isSafeInteger(length) && length > 0 ? String(length) : "";
  return `burrata-stream-audio-variant-v1\nformat=${format}\ncodec=${codec ?? ""}\nmime=${validMime ?? ""}\nvideo=${variant?.containsVideo === true ? "yes" : variant?.containsVideo === false ? "no" : "unknown"}\nlength=${lengthText}`;
}

export async function streamAudioVariantSha256(variant: StreamAudioVariant | null | undefined): Promise<string | null> {
  const identity = streamAudioVariantIdentity(variant);
  if (!identity) return null;
  const hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(identity));
  return [...new Uint8Array(hash)].map(value => value.toString(16).padStart(2, "0")).join("");
}
export type StreamMediaTrackContext = {
  id: string; provider: string; providerId: string; artist: string; title: string;
  version: string | null; album: string | null; albumArtist: string | null;
  durationSeconds: number | null; isrc: string | null; catalogNumber: string | null;
  releaseDate: string | null; recordingDate: string | null; hasPlaybackRange: boolean;
  sourcePageUrl?: string;
};
export type StreamMediaAnalysis = {
  schemaVersion: number;
  analyzerVersion: string;
  resource:
    | { kind: "registeredAsset"; recordingId: string; assetId: string; fileHash: string }
    | { kind: "stream"; recordingId: string; provider: string; sourceIdentitySha256: string; audioVariantSha256?: string | null };
  durationSeconds: number;
  scope: "wholeTrack";
  coverage: number;
  measuredAt: string;
  bpm?: unknown;
  key?: unknown;
  waveform?: {
    analyzerVersion: string;
    binCount: number;
    durationSeconds: number;
    bins: Array<WaveformBin | null>;
    scope: "wholeTrack";
    complete: boolean;
    measuredAt: string;
  } | null;
};

export function streamMediaResourceKey(resource: StreamMediaResource): string {
  return `${resource.trackId}\u0000${resource.provider}\u0000${resource.sourceId}\u0000${resource.durationSeconds.toFixed(3)}\u0000${streamAudioVariantIdentity(resource.audioVariant) ?? `session-only:${resource.sessionGeneration ?? "unknown"}`}`;
}

/** Claim at most one automatic run per exact playback resource. Manual retries
 * deliberately bypass this guard.
 */
export function claimAutomaticStreamMediaAnalysis(
  enabled: boolean,
  playing: boolean,
  resourceKey: string | null,
  attempted: Set<string>,
): boolean {
  if (!enabled || !playing || !resourceKey || attempted.has(resourceKey)) return false;
  attempted.add(resourceKey);
  return true;
}

/** Portable, bounded resolver context for an unsaved current track. It never
 * forwards media URLs, local paths, artwork URLs, provenance, or manual tags.
 */
export function streamMediaTrackContext(track: Track, playbackProvider?: string, sourceId?: string): StreamMediaTrackContext {
  const boundedText = (value: unknown, maxLength = 512): string | null =>
    typeof value === "string" && value.trim() ? value.trim().slice(0, maxLength) : null;
  const rawProvider = boundedText(track.provider, 64);
  const rawProviderId = boundedText(track.providerId);
  // Restored queue entries can omit provider fields until playback hydrates
  // them. Recover YouTube identity only from the exact canonical public page
  // and the canonical track ID; sourceId itself remains untouched for leases.
  const youtubeSourceId = playbackProvider === "youtube" && typeof sourceId === "string"
    ? (sourceId.startsWith("youtube:") ? sourceId.slice("youtube:".length) : sourceId)
    : null;
  let canonicalYoutubePage = false;
  if (youtubeSourceId && /^[A-Za-z0-9_-]{11}$/.test(youtubeSourceId) && typeof track.sourceUrl === "string") {
    try {
      const page = new URL(track.sourceUrl);
      canonicalYoutubePage = page.protocol === "https:" && !page.username && !page.password && !page.port && !page.hash &&
        ["youtube.com", "www.youtube.com"].includes(page.hostname.toLowerCase()) && page.pathname === "/watch" &&
        [...page.searchParams.keys()].length === 1 && page.searchParams.get("v") === youtubeSourceId;
    } catch { /* Restored-track inference requires a parseable canonical page. */ }
  }
  const youtubeFallback = Boolean(youtubeSourceId && /^[A-Za-z0-9_-]{11}$/.test(youtubeSourceId) &&
    canonicalYoutubePage && track.id === `youtube:${youtubeSourceId}` && (!rawProvider || rawProvider === "youtube") &&
    (!rawProviderId || rawProviderId === youtubeSourceId));
  const context: StreamMediaTrackContext = {
    id: boundedText(track.id) ?? "", provider: rawProvider ?? (youtubeFallback ? "youtube" : "unknown"),
    providerId: rawProviderId ?? (youtubeFallback ? youtubeSourceId! : boundedText(track.id)) ?? "",
    artist: boundedText(track.artist, 512) ?? "", title: boundedText(track.title, 512) ?? "",
    version: boundedText(track.version, 256), album: boundedText(track.album, 512),
    albumArtist: boundedText(track.albumArtist, 512), recordingDate: boundedText(track.recordingDate, 64),
    durationSeconds: typeof track.durationSeconds === "number" && Number.isFinite(track.durationSeconds) ? track.durationSeconds : null,
    isrc: boundedText(track.isrc, 32), catalogNumber: boundedText(track.catalogNumber, 128),
    releaseDate: boundedText(track.releaseDate, 64), hasPlaybackRange: Boolean(track.playbackRange),
  };
  if (playbackProvider && sourceId && (track.provider === playbackProvider || youtubeFallback)) {
    try {
      const page = new URL(track.sourceUrl);
      if (page.protocol === "https:" && !page.username && !page.password && !page.hash) {
        if ((playbackProvider === "youtube") && youtubeSourceId &&
            (!youtubeFallback || track.id === `youtube:${youtubeSourceId}`) && (!rawProvider || rawProvider === "youtube") &&
            ["youtube.com", "www.youtube.com"].includes(page.hostname.toLowerCase()) && page.pathname === "/watch" &&
            [...page.searchParams.keys()].length === 1 && page.searchParams.get("v") === youtubeSourceId &&
            (!rawProviderId || rawProviderId === youtubeSourceId)) {
          context.sourcePageUrl = `https://www.youtube.com/watch?v=${encodeURIComponent(youtubeSourceId)}`;
        } else if (playbackProvider === "bandcamp" && /^[a-z0-9-]+\.bandcamp\.com$/i.test(page.hostname) &&
            /^\/track\/[a-z0-9-]+\/?$/i.test(page.pathname) && page.search === "") {
          context.sourcePageUrl = `https://${page.hostname.toLowerCase()}${page.pathname.replace(/\/$/, "")}`;
        }
      }
    } catch { /* A page URL is optional resolver context; raw URLs never pass through. */ }
  }
  return context;
}

export function isCompleteStreamMediaWaveform(analysis: StreamMediaAnalysis | null | undefined): boolean {
  const waveform = analysis?.waveform;
  return Boolean(waveform && waveform.complete === true && waveform.scope === "wholeTrack" &&
    waveform.binCount === 512 && waveform.bins.length === 512 && waveform.bins.every(bin =>
      bin && Number.isFinite(bin.min) && Number.isFinite(bin.max) && bin.min >= -1 && bin.max <= 1 && bin.min <= bin.max));
}

/** A finished batch can contain a rejected recording. The full-waveform UI
 * must reflect the recording result rather than treating batch completion as
 * a successful analysis. */
export function fullStreamAnalysisOutcome(
  batchState: string,
  result: { status: string; analysis: StreamMediaAnalysis | null } | undefined,
): string {
  if (batchState !== "completed") return batchState;
  return result?.status === "completed" && isCompleteStreamMediaWaveform(result.analysis)
    ? "completed" : "failed";
}

async function sourceIdentitySha256(provider: string, sourceId: string): Promise<string> {
  const input = new TextEncoder().encode(`${provider}\n${sourceId.trim()}`);
  const hash = await crypto.subtle.digest("SHA-256", input);
  return [...new Uint8Array(hash)].map(value => value.toString(16).padStart(2, "0")).join("");
}

/** Accept a cached full waveform only when it belongs to the exact active
 * recording/provider/source ID and the measured duration matches playback.
 */
export async function verifiedStreamWaveform(
  analysis: StreamMediaAnalysis,
  resource: StreamMediaResource,
  generation: number,
): Promise<WaveformSnapshot | null> {
  const identity = analysis.resource;
  const waveform = analysis.waveform;
  if (identity.kind !== "stream" || identity.recordingId !== resource.trackId || identity.provider !== resource.provider ||
      !waveform || analysis.scope !== "wholeTrack" || !Number.isFinite(analysis.coverage) || analysis.coverage < 0.999 ||
      !Number.isFinite(resource.durationSeconds) || resource.durationSeconds < 8 || resource.durationSeconds > 900 ||
      !Number.isFinite(analysis.durationSeconds) || !Number.isFinite(waveform.durationSeconds) ||
      Math.abs(analysis.durationSeconds - resource.durationSeconds) > Math.max(0.05, resource.durationSeconds * 0.001) ||
      Math.abs(waveform.durationSeconds - analysis.durationSeconds) > 0.01 || !isCompleteStreamMediaWaveform(analysis)) return null;
  const digest = await sourceIdentitySha256(resource.provider, resource.sourceId);
  if (identity.sourceIdentitySha256 !== digest) return null;
  const variantDigest = await streamAudioVariantSha256(resource.audioVariant);
  if ((identity.audioVariantSha256 ?? null) !== variantDigest) return null;
  const snapshot = streamWaveformSnapshot({
    generation, trackId: resource.trackId, provider: resource.provider, sourceId: resource.sourceId,
    sourceIdentitySha256: digest, durationSeconds: waveform.durationSeconds,
    bins: waveform.bins, complete: waveform.complete, scope: waveform.scope,
  });
  return { ...snapshot, resource: { ...snapshot.resource!, bindingKey: streamMediaResourceKey(resource) } };
}
