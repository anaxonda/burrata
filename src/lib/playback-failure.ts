import type { PlaybackTrackFailure, Track } from "./types";

/** Terminal per-recording failures may advance an automatic queue continuation.
 * Unknown failures and shared provider/device failures keep the current entry. */
export function shouldSkipFailedTrack(reason: string): boolean {
  const message = reason.toLowerCase();
  // This resolver prefix precedes arbitrary recording metadata, which must not
  // classify a song called Connection/Network/Cancel as a global failure.
  if (message.startsWith("no playable audio match was found") || [
    "the resolved audio source identity does not match this recording",
    "source identity changed during audio resolution; select it again",
    "source identity changed during inspection; select it again",
    "this source has no supported audio stream",
  ].includes(message)) return true;
  const hasStatus = (status: string) => [`http ${status}`, `http status ${status}`, `status code ${status}`].some(value => message.includes(value));
  if (["429", "500", "502", "503", "504"].some(hasStatus) || [
    "cancel", "timeout", "timed out", "deadline", "connection", "network",
    "output device", "audio output", "service unavailable", "server unavailable",
    "temporarily unavailable", "rate limit",
  ].some(value => message.includes(value))) return false;
  return ["unavailable", "private video", "video is private", "video has been removed", "video has been deleted"].some(value => message.includes(value))
    || ["403", "404", "410"].some(hasStatus);
}

export const MAX_RECENT_TRACK_FAILURES = 64;

/** Only fixed resolver outcomes carry a category; metadata and generic errors never imply one. */
export function trackFailureCategory(reason: string): PlaybackTrackFailure["category"] {
  const message = reason.toLowerCase();
  if (/^no playable audio match was found(?: for\b|[.:]|$)/.test(message)) return "no_playable_match";
  if (["the resolved audio source identity does not match this recording", "source identity changed during audio resolution; select it again", "source identity changed during inspection; select it again"].includes(message)) return "source_identity_mismatch";
  if (message === "this source has no supported audio stream") return "unsupported_audio_stream";
  return "failed_with_reason";
}

export function safeTrackFailureIdentity(raw: string | null | undefined): string | null {
  const value = raw?.trim();
  return value && value.length <= 200 && !/[=\/\\?#\u0000-\u001f\u007f]/.test(value) && !/(?:https?|ftp|file):/i.test(value) ? value : null;
}

export function safeTrackFailureText(raw: string, maximum = 250): string {
  let redactNext = 0;
  return raw.split(/\s+/).filter(Boolean).map(part => {
    if (redactNext > 0) { redactNext--; return "[redacted]"; }
    const lower = part.toLowerCase();
    if (/(?:https?|ftp|file):\/\//i.test(part) || /^[('"]*(?:\/|[a-z]:\\|\\\\)/i.test(part)) return "[source]";
    if (/^(?:token|access[_-]?token|refresh[_-]?token|password|authorization|api[_-]?key|cookie|signature|sig)[=:]/i.test(part)) {
      if (lower.startsWith("authorization:")) redactNext = 2;
      else if (/[=:]$/.test(part)) redactNext = 1;
      return "[redacted]";
    }
    if (/^(?:bearer|token|access[_-]?token|refresh[_-]?token|password|authorization|api[_-]?key|cookie|signature|sig)$/i.test(part)) {
      redactNext = lower === "authorization" ? 2 : 1;
      return "[redacted]";
    }
    return part.replace(/[\u0000-\u001f\u007f]/g, "");
  }).join(" ").slice(0, maximum);
}

/** Called only after the queue's terminal-failure ownership guard has succeeded. */
export function appendTrackFailure(history: readonly PlaybackTrackFailure[], track: Track, owner: Pick<PlaybackTrackFailure, "queueRevision" | "generation" | "operationId" | "entryId" | "outcome">, reason: string): PlaybackTrackFailure[] {
  const entryId = safeTrackFailureIdentity(owner.entryId);
  if (!entryId) return [...history];
  const record: PlaybackTrackFailure = {
    ...owner, entryId, operationId: safeTrackFailureIdentity(owner.operationId),
    trackId: safeTrackFailureIdentity(track.id), provider: safeTrackFailureIdentity(track.provider), providerId: safeTrackFailureIdentity(track.providerId),
    title: safeTrackFailureText(track.title, 160), artist: safeTrackFailureText(track.artist, 160), version: track.version ? safeTrackFailureText(track.version, 120) : null,
    reason: safeTrackFailureText(reason), category: trackFailureCategory(reason),
  };
  return [...history, record].slice(-MAX_RECENT_TRACK_FAILURES);
}
