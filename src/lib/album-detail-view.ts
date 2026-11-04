import type { Track } from "./types";

/** Provider-neutral metadata shown by the shared saved/catalog album detail page. */
export interface AlbumDetailMetadata {
  title: string;
  artist: string;
  artistNames?: string[];
  year?: string | number | null;
  artworkUrl?: string | null;
  sourceLabel?: string | null;
  sourceUrl?: string | null;
  scopeLabel?: string | null;
  trackCountLabel?: string | null;
}

export interface AlbumDetailSelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface AlbumLoudnessAnalysisStart {
  jobId: string;
  playbackTracks: Track[];
}

export interface AlbumLoudnessAnalysisStatus {
  status: "queued" | "running" | "completed" | "failed" | "cancelled";
  completed: number;
  total: number;
  error?: string | null;
  resource?: import("./loudness-normalization").LoudnessResourceIdentity | null;
}
