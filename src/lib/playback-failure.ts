import type { PlaybackTrackFailure, Track } from "./types";

export const FIXED_RESOLVER_FAILURES = {
  noAudioResults: "No audio results were returned by the available providers. Open the catalog source or choose another source.",
  recordingIdentityUnverified: "Audio results were found, but none could be verified as the requested recording and version. Open the catalog source or choose another source.",
  missingDurationIdentityUnverified: "Audio results were found, but the saved recording has no duration and no source provided enough verified identity evidence. Open the catalog source or choose another source.",
} as const;

export type FailureCategory = "cancelled" | "timeout" | "network" | "rate_limited" | "source_access" | "source_unavailable" | "source_private" | "source_deleted" | "no_match" | "recording_identity_mismatch" | "unsupported_audio" | "output" | "storage" | "unknown";
export interface PlaybackFailure {
  category: FailureCategory;
  stage: "resolver" | "extraction" | "transport" | "output" | "storage" | "unknown";
  scope: "recording" | "source" | "global" | "operation" | "unknown";
  retryable: boolean;
  httpStatus?: number;
}

/** Compatibility policy for string IPC. Keep in parity with Rust playback_failure::classify. */
export function classifyPlaybackFailure(reason: string): PlaybackFailure {
  const message = reason.trim().toLowerCase();
  const has = (...patterns: string[]) => patterns.some(value => message.includes(value));
  const statuses = [...message.matchAll(/(?:http error |http status |http |status code |status )(\d{3})(?!\d)/g)];
  const httpStatus = statuses.map(match => Number(match[1])).find(value => value >= 400 && value <= 599);
  const failure = (category: FailureCategory, stage: PlaybackFailure["stage"], scope: PlaybackFailure["scope"], retryable: boolean, status: number | null | undefined = httpStatus): PlaybackFailure => ({category, stage, scope, retryable, ...(status == null ? {} : {httpStatus: status})});
  if (reason === FIXED_RESOLVER_FAILURES.noAudioResults || /^no playable audio match was found(?: for |[.:]|$)/.test(message)) return failure("no_match", "resolver", "recording", false, null);
  if (reason === FIXED_RESOLVER_FAILURES.recordingIdentityUnverified || reason === FIXED_RESOLVER_FAILURES.missingDurationIdentityUnverified || reason === "Verified source identity mismatch" || ["the resolved audio source identity does not match this recording", "source identity changed during audio resolution; select it again", "source identity changed during inspection; select it again"].includes(message)) return failure("recording_identity_mismatch", "resolver", "recording", false, null);
  if (message === "this source has no supported audio stream") return failure("unsupported_audio", "resolver", "recording", false, null);
  if (has("cancel", "superseded")) return failure("cancelled", "resolver", "operation", false);
  if (has("timed out", "timeout", "deadline")) return failure("timeout", "resolver", "operation", true);
  if (has("storage", "disk full", "no space left", "read-only file system", "readonly database", "database is locked", "permission denied", "local audio is not registered", "local file", "os error 28")) return failure("storage", "storage", "global", true);
  if (has("output device", "audio output", "audio device", "audio sink", "cpal")) return failure("output", "output", "global", true);
  if (httpStatus === 429 || has("rate limit", "too many requests")) return failure("rate_limited", "transport", "global", true);
  if ((httpStatus !== undefined && httpStatus >= 500) || has("network", "connection", "dns", "resolve host", "resolve hostname", "service unavailable", "server unavailable", "temporarily unavailable", "tls", "certificate")) return failure("network", "transport", "global", true);
  if (message.startsWith("audio source has no matching audio")) return failure("no_match", "extraction", "source", false);
  if (message.startsWith("audio source recording identity could not be verified")) return failure("recording_identity_mismatch", "extraction", "source", false);
  if (has("audio source is private", "audio source was removed")) return failure(message.includes("private") ? "source_private" : "source_deleted", "extraction", "source", false);
  if (httpStatus === 401 || httpStatus === 403 || has("access denied", "access was denied", "forbidden", "sign in", "sign-in", "login required", "not a bot", "challenge", "expired url", "url expired")) return failure("source_access", "extraction", "source", true);
  if (has("audio source format is unavailable", "requested format is not available", "no video formats found", "no suitable formats")) return failure("unsupported_audio", "extraction", "source", true);
  if (has("private video", "video is private")) {
    const legacyRecording = ["private video", "video is private"].includes(message);
    return failure("source_private", legacyRecording ? "resolver" : "extraction", legacyRecording ? "recording" : "source", false);
  }
  if (has("video has been removed", "video has been deleted", "video was deleted")) {
    const legacyRecording = ["video has been removed", "video has been deleted", "video was deleted"].includes(message);
    return failure("source_deleted", legacyRecording ? "resolver" : "extraction", legacyRecording ? "recording" : "source", false);
  }
  if (httpStatus === 404 || httpStatus === 410 || has("audio source is unavailable", "video unavailable", "source unavailable")) return failure("source_unavailable", "transport", "source", true);
  return failure("unknown", "unknown", "unknown", false);
}

export function shouldSkipRecording(failure: PlaybackFailure): boolean {
  return failure.scope === "recording" && !failure.retryable && ["no_match", "recording_identity_mismatch", "source_private", "source_deleted", "unsupported_audio"].includes(failure.category);
}

/** Typed detail takes precedence; the legacy reason remains supported. */
export function shouldSkipFailedTrack(reason: string, failure?: PlaybackFailure | null): boolean {
  return shouldSkipRecording(failure ?? classifyPlaybackFailure(reason));
}

export const MAX_RECENT_TRACK_FAILURES = 64;

/** Only fixed resolver outcomes carry a category; metadata and generic errors never imply one. */
export function trackFailureCategory(reason: string): PlaybackTrackFailure["category"] {
  const message = reason.toLowerCase();
  if (reason === FIXED_RESOLVER_FAILURES.noAudioResults) return "no_audio_results";
  if (reason === FIXED_RESOLVER_FAILURES.recordingIdentityUnverified || reason === FIXED_RESOLVER_FAILURES.missingDurationIdentityUnverified) return "recording_identity_unverified";
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
