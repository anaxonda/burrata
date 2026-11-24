import type { Track } from "./types";
import type { ArtistRef } from "./artist-types";
import { classicalWorkPlacement } from "./classical-works";

export const OPEN_CREDIT_ARTIST = Symbol("open credited artist");

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

export function isUpcomingYouTubeUpload(track: Track): boolean {
  return track.provider === "youtube" && object(track.provenance).youtubeUpcoming === true;
}

export function youtubeFeaturedArtist(track: Track): string | null {
  if (track.provider !== "youtube" || track.artist.trim()) return null;
  const provenance = object(track.provenance);
  if (provenance.available === false || object(provenance.manualFields).artist === true) return null;
  const channelId = provenance.uploaderChannelId;
  const channelName = provenance.channelName ?? provenance.uploaderChannelTitle;
  // Older playlist snapshots only carry the channel label. This is a search
  // hint even when the stable channel ID is present, never a recording credit.
  const isNpr = channelId == null
    ? typeof channelName === "string" && channelName.trim().toLowerCase() === "npr music"
    : channelId === "UC4eYXhJI4-7wSWc8UNRwD4A";
  if (!isNpr) return null;
  const name = /^(?<name>[^\p{Cc}]{1,120}?):\s+Tiny Desk(?: \(Home\))? Concert$/iu
    .exec(track.title.trim())?.groups?.name.trim();
  return name && /\p{L}/u.test(name) ? name : null;
}

const roleLabels: Record<string, string> = {
  composer: "Composer", orchestra: "Orchestra", conductor: "Conductor",
  instrument: "Instrumentalist", vocal: "Vocalist", performer: "Performer",
};

export function classicalTrackDisplay(track: Track) {
  if (track.provider === "local") {
    const placement = classicalWorkPlacement(track);
    return placement?.movement ? { workTitle: placement.title, movement: placement.movement, credits: [] } : null;
  }
  const provenance = object(track.provenance);
  if (track.provider !== "musicbrainz" &&
      !(track.provider === "catalog" && provenance.catalogProvider === "musicbrainz")) return null;
  const work = object(provenance.classicalWork);
  const relations = Array.isArray(provenance.recordingRelations) ? provenance.recordingRelations.slice(0, 64) : [];
  if (work.classical !== true && !relations.some(row =>
    ["composer", "conductor", "orchestra", "performing orchestra"].includes(String(object(row).type)))) return null;
  const text = (value: unknown) => typeof value === "string" ? value.trim().slice(0, 512) : "";
  const workTitle = text(work.title);
  const manual = object(provenance.manualFields);
  const movement = manual.title === true ? "" : text(work.movement);
  const credits: { name: string; artist: ArtistRef; role: string }[] = [];
  function addCredit(raw: unknown, role: string) {
    const credit = object(raw);
    const id = text(credit.id), name = text(credit.name);
    if (!name || !role || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(id) ||
        credits.some(existing => existing.artist.id === id && existing.role === role)) return;
    credits.push({ name, artist: { provider: "musicbrainz", id }, role });
  }
  if (Array.isArray(work.composers)) {
    for (const composer of work.composers.slice(0, 16)) addCredit(composer, "Composer");
  }
  for (const role of ["composer", "orchestra", "instrument", "vocal", "performer", "conductor"]) {
    for (const row of relations) {
      const relation = object(row);
      if (relation.type === role || (role === "orchestra" && relation.type === "performing orchestra")) {
        addCredit(relation.artist, roleLabels[role]);
      }
    }
  }
  const catalog = object(provenance.catalog);
  const rawCredits = Array.isArray(provenance.artistCredits) ? provenance.artistCredits : catalog.artists;
  if (Array.isArray(rawCredits)) {
    for (const row of rawCredits.slice(0, 32)) {
      const credit = object(row);
      const artist = object(credit.artist ?? row);
      if (artist.provider === "musicbrainz" && !credits.some(existing => existing.artist.id === artist.id)) {
        addCredit({ id: artist.id, name: credit.name }, "Artist");
      }
    }
  }
  return movement || credits.length ? { workTitle, movement, credits } : null;
}

function titleKey(value: string): string {
  return value.normalize("NFKC").toLowerCase().replace(/[^\p{L}\p{M}\p{N}]+/gu, " ").trim();
}

export function trackDisplayVersion(track: Track): string {
  const version = track.version?.trim() ?? "";
  const manual = (track.provenance as { manualFields?: { version?: boolean } } | null)?.manualFields;
  if (!version || manual?.version) return version;

  const titleSuffix = /\(([^()]*)\)$|\[([^\[\]]*)\]$|\s[-–—]\s(.+)$/.exec(track.title.trim());
  const suffix = titleSuffix?.[1] ?? titleSuffix?.[2] ?? titleSuffix?.[3];
  if (suffix && titleKey(suffix) === titleKey(version)) return "";
  if (track.provider !== "traxsource") return version;

  const clock = /(?:^|\s)\((\d{1,3}):(\d{2})\)$/.exec(version);
  const runtime = clock ? Number(clock[1]) * 60 + Number(clock[2]) : 0;
  const label = clock && runtime > 0 && Math.abs((track.durationSeconds ?? 0) - runtime) < 0.5
    ? version.slice(0, clock.index).trimEnd()
    : version;
  const repeatedTitle = /^(.*?)\s+\[([^\[\]]+)\]$/.exec(label);
  if (repeatedTitle && titleKey(repeatedTitle[1]) === titleKey(track.title.replace(/\s+\(remix\)$/i, ""))) {
    return repeatedTitle[2].trim();
  }
  return label;
}
