export type AutomationSearchKind = "all" | "tracks" | "artists" | "albums" | "videos" | "playlists" | "profiles" | "channels";
export type AutomationSearchSourceId = "all" | "library" | "musicbrainz" | "discogs" | "youtube" | "soundcloud" | "soulseek" | "monochrome" | "spotify";
export type AutomationSearchStatus = "complete" | "failed" | "cancelled";
export type AutomationSearchErrorCode = "no_sources" | "source_unavailable" | "request_failed" | "cancelled" | "invalid_request";

export type AutomationSearchRequest = {
  requestId: string;
  query: string;
  sourceId: AutomationSearchSourceId;
  kind: AutomationSearchKind;
};

/** A bounded summary of the result visible in SearchPanel; it never includes query text or result rows. */
export type AutomationSearchReport = {
  requestId: string;
  status: AutomationSearchStatus;
  sourceId: AutomationSearchSourceId;
  kind: AutomationSearchKind;
  resultCounts: Record<string, number>;
  /** Per-source continuation and exhaustiveness; omitted by older clients. */
  sourceCoverage?: Record<string, { hasMore: boolean; incomplete: boolean }>;
  errorCode?: AutomationSearchErrorCode;
};
