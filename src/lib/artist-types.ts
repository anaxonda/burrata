import type { Track } from "./types";
import type { TrackSortField, SortDirection } from "./track-sort";

export type ArtistProvider = "musicbrainz" | "discogs" | "monochrome";

export interface ArtistRef {
  provider: ArtistProvider;
  id: string;
}

export interface ArtistSearchResult {
  artist: ArtistRef;
  name: string;
  sortName?: string | null;
  disambiguation?: string | null;
  artistType?: string | null;
  country?: string | null;
  beginDate?: string | null;
  endDate?: string | null;
  score?: number | null;
  sourceUrl: string;
}

export interface ArtistSearchPage {
  query: string;
  page: number;
  pageSize: number;
  total?: number | null;
  hasMore: boolean;
  results: ArtistSearchResult[];
  warnings: string[];
}

export interface ArtistExternalLink {
  label: string;
  url: string;
  provider?: string | null;
}

export interface ArtistCredit {
  name: string;
  artist?: ArtistRef | null;
  role: string;
  creditedAs?: string | null;
  joinPhrase?: string | null;
  sourceUrl: string;
}

export interface ArtistReleaseRef {
  release: ArtistRef;
  title: string;
  year?: number | null;
  releaseDate?: string | null;
  country?: string | null;
  label?: string | null;
  catalogNumber?: string | null;
  format?: string | null;
  primaryType?: string | null;
  trackCount?: number | null;
  tracks: ArtistReleaseTrack[];
  artworkUrls?: string[];
  sourceUrl: string;
  credits: ArtistCredit[];
}

export interface ArtistReleaseTrack {
  position?: string | null;
  title: string;
  sourceUrl: string;
}

export interface ArtistProfile {
  artist: ArtistSearchResult;
  biography?: string | null;
  biographySource?: string | null;
  imageUrl?: string | null;
  genres: string[];
  tags: string[];
  sourceTags: ArtistTag[];
  aliases: string[];
  links: ArtistExternalLink[];
  topTracks: Track[];
  releases: ArtistReleasePage;
  discogsAvailable: boolean;
  lastfmAvailable: boolean;
  sourceCoverage: ArtistSourceCoverage[];
  warnings: string[];
}

export interface ArtistTag {
  name: string;
  provider: string;
  kind: string;
}

export interface ArtistSourceCoverage {
  provider: string;
  status: string;
  detail: string;
  sourceUrl?: string | null;
}

export interface ArtistReleasePage {
  artist: ArtistRef;
  items: ArtistReleaseRef[];
  nextCursor?: string | null;
  sourceCoverage: ArtistSourceCoverage[];
  warnings: string[];
}

export interface AlbumSearchPage {
  items: ArtistReleaseRef[];
  nextPage?: number | null;
  warnings: string[];
}

export interface ArtistTrackPage {
  artist: ArtistRef;
  items: Track[];
  nextCursor?: string | null;
  source: string;
  warnings: string[];
}

export interface ArtistCollaboration {
  credits: ArtistCredit[];
  description?: string | null;
  title?: string | null;
  date?: string | null;
  sourceUrl: string;
}

export interface ArtistCollaborationPage {
  artist: ArtistRef;
  items: ArtistCollaboration[];
  nextCursor?: string | null;
  sourceCoverage: ArtistSourceCoverage[];
  warnings: string[];
}

export interface ArtistRecordingMatch {
  recording: ArtistRef;
  title: string;
  artist: string;
  score: number;
  firstReleaseDate?: string | null;
  isrcs: string[];
  sourceUrl: string;
}

export type MetadataFieldKey =
  | "artist"
  | "title"
  | "version"
  | "genre"
  | "isrc"
  | "label"
  | "catalogNumber"
  | "artworkUrl"
  | "releaseDate";

export interface MetadataReviewField {
  key: MetadataFieldKey;
  value: string | null;
  source: string;
  sourceUrl: string;
  reviewRequired: boolean;
  note?: string | null;
}

export interface MetadataReview {
  sourceTrackId: string;
  recording?: ArtistRecordingMatch | null;
  recordingCandidates: ArtistRecordingMatch[];
  releaseRef?: ArtistReleaseRef | null;
  releaseTracklist: ArtistReleaseTrack[];
  matchedReleaseTrack?: string | null;
  fields: MetadataReviewField[];
  credits: ArtistCredit[];
  recordingFirstReleaseDate?: string | null;
  releaseDate?: string | null;
  releaseYear?: number | null;
  selectedVersionPreserved?: string | null;
  warnings: string[];
}

export interface ArtistLocalAssetsPageRequest {
  names: string[];
  limit: number;
  offset: number;
  query: string;
  sortField: TrackSortField;
  direction: SortDirection;
  minDurationSeconds?: number;
  maxDurationSeconds?: number;
  availability: "all" | "on-device" | "not-on-device";
}

export interface ArtistLocalAssetsPage {
  items: Track[];
  total: number;
  limit: number;
  offset: number;
  sortAvailability?: Partial<Record<TrackSortField, boolean>> | null;
}

export interface ArtistPageProps {
  activeTrack: Track | null;
  artistName?: string | null;
  initialArtistRef?: ArtistRef | null;
  initialSection?: "overview" | "releases" | "tracks" | "collaborations" | "local";
  initialScroll?: number;
  onSectionChange?: (section: "overview" | "releases" | "tracks" | "collaborations" | "local") => void;
  onArtistSelected?: (artist: ArtistRef | null, name?: string) => void;
  onScroll?: (position: number) => void;
  localTracks: Track[];
  onLoadLocalAssetsPage?: (request: ArtistLocalAssetsPageRequest) => Promise<ArtistLocalAssetsPage>;
  searchArtists: (query: string, page: number) => Promise<ArtistSearchPage>;
  getArtistProfile: (artist: ArtistRef) => Promise<ArtistProfile>;
  getArtistReleases: (artist: ArtistRef, cursor?: string | null) => Promise<ArtistReleasePage>;
  getReleaseTracks?: (release: ArtistReleaseRef, artist: ArtistRef) => Promise<Track[]>;
  getArtistTracks: (artist: ArtistRef, cursor?: string | null) => Promise<ArtistTrackPage>;
  getArtistCollaborations: (artist: ArtistRef, cursor?: string | null) => Promise<ArtistCollaborationPage>;
  resolveRecordingMetadata: (track: Track, release?: ArtistReleaseRef | null, recording?: ArtistRef | null) => Promise<MetadataReview>;
  onApplyMetadata: (track: Track, review: MetadataReview, acceptedFieldKeys: MetadataFieldKey[]) => void | Promise<void>;
  onPlayTrack?: (track: Track, displayedTracks: Track[], index: number) => void | Promise<void>;
  onQueueTracks?: (tracks: Track[], mode: "append" | "next") => void | Promise<void>;
  onTrackContext?: (track: Track, event: MouseEvent) => void;
  onDownloadTrack?: (track: Track) => void | Promise<void>;
  onFavoriteTrack?: (track: Track) => void | Promise<void>;
  onAddTrackToPlaylist?: (track: Track) => void | Promise<void>;
  onAddSelectedToPlaylist?: (tracks: Track[]) => void | Promise<void>;
  onArtistNavigate?: (artist: ArtistRef) => void | Promise<void>;
  playingTrack?: Track | null;
  bufferingTrack?: Track | null;
  playingTrackId?: string;
  bufferingTrackId?: string;
  paused?: boolean;
}
