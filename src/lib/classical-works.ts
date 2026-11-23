import type { Track } from "./types";

type Metadata = Record<string, unknown>;

export interface WorkPlacement {
  key: string;
  title: string;
  composers: string[];
  movement: string;
  part: number | null;
  partTotal: number | null;
  partId: string | null;
  performanceKey: string | null;
  performanceLabel: string;
  releaseKey: string;
  source: "catalog" | "fileTags";
}

export interface WorkProgramme {
  key: string;
  title: string;
  composers: string[];
  performanceLabel: string;
  tracks: Track[];
  startIndex: number;
  complete: boolean;
  source: WorkPlacement["source"];
}

export interface ClassicalWork {
  key: string;
  title: string;
  composers: string[];
  tracks: Track[];
  source: WorkPlacement["source"];
}

export type ClassicalTrackListRow =
  | { kind: "track"; key: string; trackIndex: number }
  | { kind: "work"; key: string; programme: WorkProgramme };

function record(value: unknown): Metadata {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Metadata : {};
}

function text(value: unknown): string {
  if (typeof value !== "string" || value.length > 512 || /[\u0000-\u001f\u007f]/.test(value)) return "";
  return value.trim();
}

function mbid(value: unknown): string {
  const id = text(value);
  return /^[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i.test(id) ? id.toLowerCase() : "";
}

function ordinal(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= 512 ? value : null;
}

function fold(value: string): string {
  return value.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim();
}

function uniqueNames(values: unknown[]): string[] {
  return [...new Set(values.map(text).filter(Boolean))];
}

function catalogPlacement(track: Track, metadata: Metadata): WorkPlacement | null {
  const catalog = record(metadata.catalog);
  const recordingId = mbid(metadata.recordingId) || mbid(catalog.recordingId);
  const releaseId = mbid(metadata.releaseId);
  const releaseUrl = `https://musicbrainz.org/release/${releaseId}`;
  const recordingSource = track.provider === "musicbrainz" && track.sourceUrl === `https://musicbrainz.org/recording/${recordingId}`
    && track.id === `musicbrainz:${recordingId}` && track.providerId === recordingId;
  const releaseSource = track.provider === "catalog" && metadata.sourceKind === "catalog" && metadata.catalogProvider === "musicbrainz"
    && releaseId && track.sourceUrl === releaseUrl && track.id === `catalog:${track.providerId}`
    && track.providerId.startsWith(`musicbrainz:${releaseId}:`) && track.providerId.endsWith(`:${recordingId}`);
  const work = record(metadata.classicalWork);
  const partId = mbid(work.id);
  const parentId = mbid(work.parentId);
  const title = text(work.title);
  if (!recordingId || (!recordingSource && !releaseSource) || work.classical !== true || !partId || !title) return null;
  // An old payload may show the parent's title without its ID. Exclude it
  // from the work index until the catalog supplies an unambiguous identifier.
  if (text(work.movement) && !parentId) return null;

  const roles = Array.isArray(metadata.recordingRelations) ? metadata.recordingRelations.slice(0, 64) : [];
  const performers = new Map<string, string>();
  for (const raw of roles) {
    const relation = record(raw), artist = record(relation.artist);
    const id = mbid(artist.id), name = text(artist.name);
    const role = relation.type === "performing orchestra" ? "orchestra" : text(relation.type);
    if (id && name && ["orchestra", "conductor", "performer", "instrument", "vocal"].includes(role)) {
      performers.set(`${role}:${id}`, name);
    }
  }
  const event = mbid(work.performanceEventId);
  const place = mbid(work.performancePlaceId);
  const date = text(work.performanceDate);
  const performanceIdentity = event ? `event:${event}` : place && /^\d{4}-\d{2}-\d{2}$/.test(date) ? `place:${place}:${date}` : "";
  // An event or a recorded date and place binds the programme. Matching
  // performers by name, or even matching artist IDs alone, is insufficient.
  const performanceKey = releaseSource && performanceIdentity && performers.size
    ? JSON.stringify([performanceIdentity, [...performers.keys()].sort()]) : null;
  return {
    key: `musicbrainz:${parentId || partId}`,
    title,
    composers: uniqueNames(Array.isArray(work.composers) ? work.composers.slice(0, 16).map(value => record(value).name) : []),
    movement: text(work.movement),
    part: parentId ? ordinal(work.partNumber) : null,
    partTotal: parentId ? ordinal(work.partTotal) : null,
    partId: parentId ? partId : null,
    performanceKey,
    performanceLabel: uniqueNames([...performers.values()]).join(" · ") || track.artist,
    releaseKey: releaseSource ? releaseUrl : "",
    source: "catalog",
  };
}

function taggedPlacement(track: Track, metadata: Metadata): WorkPlacement | null {
  if (track.provider !== "local" || !track.localPath || metadata.source !== "local file tags") return null;
  const tags = record(metadata.embeddedClassical);
  const title = text(tags.work), movement = text(tags.movement);
  if (tags.version !== 1 || !title) return null;
  const album = text(track.album);
  // MusicBrainz Work Id tags frequently identify the movement. Keep that
  // identifier descriptive and scope titles to their real album/file folder.
  const localReleaseId = mbid(record(metadata.embeddedIdentifiers).musicbrainzReleaseId);
  const folder = track.localPath.replace(/\\/g, "/").replace(/\/[^/]*$/, "");
  const releaseKey = album ? JSON.stringify([localReleaseId || folder, album, text(track.albumArtist)]) : "";
  const composers = uniqueNames(Array.isArray(tags.composers) ? tags.composers.slice(0, 16) : []);
  const part = ordinal(tags.movementNumber), partTotal = ordinal(tags.movementTotal);
  const performer = text(track.artist);
  return {
    key: JSON.stringify(["fileTags", releaseKey || track.id, fold(title), composers.map(fold)]),
    title,
    composers,
    movement,
    part,
    partTotal,
    partId: null,
    performanceKey: releaseKey && part && partTotal && performer && movement ? performer : null,
    performanceLabel: performer,
    releaseKey,
    source: "fileTags",
  };
}

export function classicalWorkPlacement(track: Track): WorkPlacement | null {
  if (track.playbackRange) return null;
  const metadata = record(track.provenance);
  const manual = record(metadata.manualFields);
  if (manual.title === true || manual.artist === true || manual.album === true) return null;
  return catalogPlacement(track, metadata) ?? taggedPlacement(track, metadata);
}

/** Group only consecutive album occurrences with a common work and evidenced performance. */
export function classicalAlbumProgrammes(tracks: Track[]): WorkProgramme[] {
  const programmes: WorkProgramme[] = [];
  let current: { placement: WorkPlacement; tracks: Track[]; startIndex: number; last: number; first: number; parts: Set<string> } | null = null;

  function finish() {
    if (!current) return;
    const { placement, tracks, startIndex, first, last } = current;
    const complete = first === 1 && last === placement.partTotal;
    if (tracks.length > 1 && (placement.source === "catalog" || complete)) {
      programmes.push({
        key: JSON.stringify([placement.key, placement.releaseKey, placement.performanceKey, startIndex]),
        title: placement.title, composers: placement.composers, performanceLabel: placement.performanceLabel,
        tracks, startIndex, complete, source: placement.source,
      });
    }
    current = null;
  }

  tracks.forEach((track, index) => {
    const placement = classicalWorkPlacement(track);
    const part = placement?.part;
    if (!placement?.performanceKey || !placement.releaseKey || !part || (placement.partTotal && part > placement.partTotal)) {
      finish();
      return;
    }
    if (current && (placement.key !== current.placement.key
        || placement.releaseKey !== current.placement.releaseKey
        || placement.performanceKey !== current.placement.performanceKey
        || placement.partTotal !== current.placement.partTotal
        || part !== current.last + 1
        || Boolean(placement.partId && current.parts.has(placement.partId)))) finish();
    if (!current) current = { placement, tracks: [], startIndex: index, first: part, last: part, parts: new Set() };
    current.tracks.push(track);
    current.last = part;
    if (placement.partId) current.parts.add(placement.partId);
  });
  finish();
  return programmes;
}

export function classicalTrackListRows(tracks: Track[], keys?: string[]): ClassicalTrackListRow[] | undefined {
  const programmes = classicalAlbumProgrammes(tracks);
  if (!programmes.length) return undefined;
  const groups = new Map(programmes.map(programme => [programme.startIndex, programme]));
  return tracks.flatMap((track, trackIndex): ClassicalTrackListRow[] => {
    const row = { kind: "track" as const, key: keys?.[trackIndex] ?? `${track.id}:${trackIndex}`, trackIndex };
    const programme = groups.get(trackIndex);
    return programme ? [{ kind: "work", key: `work:${programme.key}`, programme }, row] : [row];
  });
}

/** Restore provider album order even when library tracks were saved out of order. */
export function classicalWorkTracksInAlbumOrder(tracks: Track[]): Track[] {
  return tracks.map((track, index) => {
    const metadata = record(track.provenance);
    const placement = classicalWorkPlacement(track);
    return {
      track, index, releaseKey: placement?.releaseKey ?? "",
      disc: ordinal(metadata.mediumPosition) ?? ordinal(track.discNumber) ?? 1,
      position: ordinal(metadata.trackPositionOrdinal) ?? ordinal(track.trackNumber) ?? index + 1,
    };
  }).sort((a, b) => a.releaseKey.localeCompare(b.releaseKey) || a.disc - b.disc || a.position - b.position || a.index - b.index)
    .map(row => row.track);
}

export function findClassicalWorks(tracks: Track[], query = ""): ClassicalWork[] {
  const entries = new Map<string, ClassicalWork>();
  for (const track of tracks) {
    const placement = classicalWorkPlacement(track);
    if (!placement) continue;
    const existing = entries.get(placement.key);
    if (existing) {
      existing.tracks.push(track);
      existing.composers = uniqueNames([...existing.composers, ...placement.composers]);
    } else entries.set(placement.key, {
      key: placement.key, title: placement.title, composers: placement.composers, tracks: [track], source: placement.source,
    });
  }
  return filterClassicalWorks(
    [...entries.values()].sort((a, b) => a.title.localeCompare(b.title) || a.key.localeCompare(b.key)),
    query,
  );
}

export function filterClassicalWorks(works: ClassicalWork[], query: string): ClassicalWork[] {
  const words = fold(query).split(" ").filter(Boolean);
  return works.filter(work => {
    const searchable = fold([work.title, ...work.composers].join(" "));
    const terms = searchable.split(" ");
    return words.every(word => /^\d+$/.test(word) ? terms.includes(word) : searchable.includes(word));
  });
}
