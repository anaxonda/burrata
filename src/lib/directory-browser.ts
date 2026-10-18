import type { Track } from "./types";

export interface DirectoryBrowserRoot {
  id: string;
  name: string;
  path: string;
}

export interface DirectoryBreadcrumb {
  name: string;
  relativePath: string;
}

export interface DirectoryChild {
  name: string;
  relativePath: string;
}

/** A filesystem occurrence. Multiple files may point at the same recording. */
export interface DirectoryAudioFile {
  path: string;
  name: string;
  assetId?: string | null;
  track?: Track | null;
}

export interface DirectoryBrowsePage {
  rootId: string;
  path: string;
  relativePath: string;
  breadcrumbs: DirectoryBreadcrumb[];
  directories: DirectoryChild[];
  files: DirectoryAudioFile[];
  total: number;
  /** Ten fields in shared track_sort_sql order; absent on the fast filename-only page. */
  sortAvailability?: DirectorySortAvailabilityFields | null;
  offset: number;
  limit: number;
}

export type DirectorySortAvailabilityFields = readonly [boolean, boolean, boolean, boolean, boolean, boolean, boolean, boolean, boolean, boolean];
export interface DirectorySortAvailabilityJob {
  jobId: string; rootId: string; relativePath: string;
  state: "running" | "completed" | "cancelled" | "failed";
  completed: number; total?: number | null; fields?: DirectorySortAvailabilityFields | null; error?: string | null;
}

export interface DirectoryScanJob {
  jobId: string;
  rootId: string;
  state: "running" | "completed" | "cancelled" | "failed";
  completed: number;
  indexed: number;
  failed: number;
  total?: number | null;
  error?: string | null;
}
