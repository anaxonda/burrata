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
  audioCodec?:string; audioBitrateKbps?:number;
  sessionId?: string; provider?: string; title?: string;
  kind: string;
  url: string;
  canSeek: boolean;
  canVolume: boolean;
  canAutoplay: boolean;
  isExcerpt: boolean;
}
export interface AudioFormat {
  id: string;
  codec: string;
  extension: string;
  bitrate?: number | null;
  sampleRate?: number | null;
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
  previewDelayMs: number;
  theme: string;
  ytDlpPath: string;
  ffmpegPath: string;
  ffprobePath: string;
  jsRuntimePath: string;
  preferredDownloadSource?: string;
  preferredStreamingSource?: string; overridePlaylistTrackUrls?: boolean;
  streamingFallbackEnabled?: boolean;
  streamingFallbackSources?: string[];
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
