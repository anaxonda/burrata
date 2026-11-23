import type { Track } from "./types";

export type SubsonicStatus = {
  endpoint: string | null;
  username: string | null;
  visible: boolean;
  allowFallback: boolean;
  trackCount: number;
  canSeed: boolean;
  strawberryFound: boolean;
  refreshAfterLogin?: boolean;
  warning?: string | null;
};

export type SubsonicPage = { tracks: Track[]; total: number };
