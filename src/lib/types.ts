import type { PlaybackFailure } from "./playback-failure";

export interface Track {
  id: string;
  provider: string;
  providerId: string;
  sourceUrl: string;
  artist: string;
  title: string;
  version?: string | null;
  album?: string | null;
  albumArtist?: string | null;
  recordingDate?: string | null;
  trackNumber?: number | null;
  trackTotal?: number | null;
  discNumber?: number | null;
  discTotal?: number | null;
  remixers?: string[] | null;
  playbackRange?: { parentTrackId: string; chapterId: string; sourceUrl: string; startTime: number; endTime: number; parentTitle: string; parentDurationSeconds?: number | null };
  label?: string | null;
  genre?: string | null;
  normalizedGenre?: string | null;
  durationSeconds?: number | null;
  bpm?: number | null;
  key?: string | null;
  releaseDate?: string | null;
  /** Raw legacy JSON; sort normalization treats non-string values as unknown. */
  dateAdded?: unknown;
  artworkUrl?: string | null;
  isrc?: string | null;
  catalogNumber?: string | null;
  rank?: number | null;
  previewUrl?: string | null;
  favorite: boolean;
  disposition?: string | null;
  localPath?: string | null;
  localAvailability?: "none" | "available" | "missing" | string;
  assetId?: string | null;
  provenance: unknown;
}
export interface Genre {
  id: string;
  name: string;
  chartUrl: string;
}
export interface Chart {
  id: string;
  provider: string;
  title: string;
  sourceUrl: string;
  fetchedAt: string;
  tracks: Track[];
  stale: boolean;
  warning?: string | null;
}
export interface BrowserStatus {
  connected: boolean; pendingCount: number; lastSeen: string | null; hostName: string;
}
export interface BrowserImportResult {
  id: string; provider: string; kind: "chart" | "playlist" | "album" | "collection"; title: string; sourceUrl: string; fetchedAt: string; collectionId: string; capturedCount: number; importedCount: number; expectedTotal: number | null; complete: boolean; warnings: string[];
}
export interface CandidatePreviewStatus {
  sessionId: string; sourceId: string; provider: string; paused: boolean; position: number; duration: number; canSeek: boolean; canVolume: boolean; error?: string | null; closed?: boolean;
}
export interface Preview {
  matching?: {evaluatorVersion:string;level:"exact"|"balanced"|"flexible";assessment:{relationship:"exact"|"probable"|"alternative"|"unresolved"};resolvedRecording:{title:string;artist?:string|null;version?:string|null;sourceId:string}};
  sourceTransport?: string;
  sourceHeaderPolicy?: {headerCount:number;authorization:boolean;cookie:boolean;referer:boolean;origin:boolean;userAgent:boolean;other:boolean};
  sourceGeneration?: number;
  audioCodec?:string; audioBitrateKbps?:number; audioMimeType?:string;
  audioFormatId?:string; audioFormatHasVideo?:boolean; audioExpectedContentLength?:number;
  sessionId?: string; provider?: string; title?: string;
  /** Resolved media duration in seconds, when the source has a stable duration. */
  durationSeconds?: number | null;
  kind: string;
  url: string;
  canSeek: boolean;
  canVolume: boolean;
  canAutoplay: boolean;
  isExcerpt: boolean;
}
export interface NativePlaybackSnapshot {
  failure?: PlaybackFailure | null;
  engine: "native" | "webview";
  revision: number;
  naturalQueueEnd?: boolean;
  diagnostics?: {
    lastSourceFailure?: NativePlaybackFailure | null;
    lastSkippedTrackFailure?: NativePlaybackFailure | null;
    recentTrackFailures?: PlaybackTrackFailure[];
  };
  generation: number;
  status: "idle" | "resolving" | "buffering" | "playing" | "paused" | "ended" | "failed";
  trackId: string | null;
  track: Track | null;
  candidate: Candidate | null;
  queue: Track[];
  queueRevision?: number;
  currentIndex: number;
  paused: boolean;
  position: number;
  duration: number;
  volume: number;
  canSeek: boolean;
  canVolume: boolean;
  shuffle: boolean;
  repeat: "off" | "all" | "one" | string;
  playerMessage: string;
  quality: { assetId?: string; codec?: string; bitrateKbps?: number; sampleRate?: number; channels?: number; bitDepth?: number; bitrateOrigin?: string };
  chapter?: { title: string; startSeconds: number; endSeconds?: number } | null;
  radioNowPlaying?: { id: string; title: string; artist: string; stationName: string; metadata?: Record<string, unknown> | null } | null;
  radioHistory?: { id: string; track: Track; playedAt: string; stationName: string }[];
}
export interface NativePlaybackFailure {
  trackId: string | null;
  provider?: string | null;
  providerId?: string | null;
  title: string;
  artist: string;
  reason: string;
}
/** Ephemeral bounded failure history; entryId identifies duplicate queue occurrences. */
export interface PlaybackTrackFailure extends NativePlaybackFailure {
  queueRevision: string | number;
  generation: number;
  operationId: string | null;
  entryId: string;
  outcome: "skipped" | "queue_exhausted";
  category: "no_playable_match" | "no_audio_results" | "recording_identity_unverified" | "source_identity_mismatch" | "unsupported_audio_stream" | "failed_with_reason";
  version?: string | null;
}
export interface AudioFormat {
  id: string;
  codec: string;
  extension: string;
  bitrate?: number | null;
  sampleRate?: number | null;
  bitDepth?: number | null;
  channels?: number | null;
  sizeBytes?: number | null;
  lossless: boolean;
}
export interface Chapter { id: string; title: string; startTime: number; endTime: number; confidence: string; suggestedTrack?: Track | null; suggestionConfidence?: string | null; }
export interface Candidate {
  sourceMetadata?: Record<string, unknown> | null;
  chapters?: Chapter[] | null;
  id: string;
  provider: string;
  url: string;
  title: string;
  uploader?: string | null;
  durationSeconds?: number | null;
  artworkUrl?: string | null;
  formats: AudioFormat[];
  permission: string;
  confidence: string;
  score: number;
  reasons: string[];
  official: boolean;
  source?: {
    kind: "soulseek";
    username: string;
    filename: string;
    size_bytes: number;
    bitrate_kbps?: number | null;
    search_id?: string | null;
    batch_id?: string | null;
    transfer_id?: string | null;
  } | null;
}
export interface DownloadRequest {
  chapterIds?: string[] | null;
  track: Track;
  candidate: Candidate;
  formatId: string | null;
  outputProfile: string;
}
export interface DownloadJob {
  soulseek?: {
    bytesReceived: number;
    lastProgressAt?: string | null;
    speedBytesPerSecond: number;
    queuePosition?: number | null;
    queueCheckedAt?: string | null;
    queueError?: string | null;
    attempts: number;
    nextAttemptAt?: string | null;
    daemonState: string;
  } | null;
  chapterIds?: string[] | null;
  id: string;
  track: Track;
  candidate: Candidate;
  state: string;
  progress: number;
  error?: string | null;
  outputPath?: string | null;
  createdAt: string;
  outputProfile: string;
  formatId?: string | null;
}
export interface Collection {
  id: string;
  name: string;
  trackCount: number;
}
export interface Settings {
  musicDirectory: string;
  autoPreview: boolean;
  matchingLevel?: "exact" | "balanced" | "flexible";
  pauseOnSourceOpen?: boolean;
  sourceOpenAtPosition?: boolean;
  previewDelayMs: number;
  playbackStartupTimeoutSeconds?: 20 | 30 | 45;
  theme: string;
  ytDlpPath: string;
  ffmpegPath: string;
  ffprobePath: string;
  jsRuntimePath: string;
  preferredDownloadSource?: string;
  preferredStreamingSource?: string; overridePlaylistTrackUrls?: boolean;
  streamingFallbackEnabled?: boolean;
  streamingFallbackDelayMs?: number;
  streamingFallbackSources?: string[];
  bandcampFallbackEnabled?: boolean;
  backgroundSourcePreparation?: boolean;
  downloadFallbackEnabled?: boolean;
  downloadFallbackSources?: string[];
  downloadFallbackAllowLossy?: boolean;
  playlistDirectories?: string[];
}
export interface Dependency {
  name: string;
  available: boolean;
  version?: string | null;
  message: string;
}
export interface LocalAsset {
  id: string;
  recordingId: string;
  path: string;
  hash: string;
  sourceCodec: string;
  finalCodec: string;
  sourceBitrate?: number | null;
  finalBitrate?: number | null;
  sampleRate?: number | null;
  channels?: number | null;
  bitDepth?: number | null;
  durationSeconds?: number | null;
  sizeBytes?: number | null;
  bitrateOrigin?: string | null;
  processing: string;
  track: Track;
}
export interface ChartMembership {
  provider: string;
  title: string;
  sourceUrl: string;
  rank: number | null;
  fetchedAt: string;
}
