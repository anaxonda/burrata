import type { Track } from "./types";

type ArtworkImage = { url: string; width: number; height: number };
export type ArtworkUpgradeCandidate = ArtworkImage;

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function validMbid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}

function spotifyImageCandidates(track: Track, provenance: Record<string, unknown>): ArtworkImage[] {
  const catalog = isRecord(provenance.catalog) ? provenance.catalog : null;
  const remote = isRecord(provenance.remoteCatalog) ? provenance.remoteCatalog : null;
  if (!catalog || !remote || catalog.provider !== "spotify" || catalog.providerId !== track.providerId) return [];
  const rawImages = Array.isArray(remote.images) ? remote.images : isRecord(remote.album) && Array.isArray(remote.album.images) ? remote.album.images : [];
  const images: ArtworkImage[] = rawImages.flatMap((raw): ArtworkImage[] => {
    if (!isRecord(raw) || typeof raw.url !== "string" || raw.url.length > 2048) return [];
    let url: URL;
    try { url = new URL(raw.url); } catch { return []; }
    const width = raw.width;
    const height = raw.height;
    if (url.protocol !== "https:" || url.hostname !== "i.scdn.co" || url.username || url.password || url.port ||
      !Number.isSafeInteger(width) || !Number.isSafeInteger(height) || (width as number) < 1 || (height as number) < 1 ||
      (width as number) > 10_000 || (height as number) > 10_000) return [];
    return [{ url: raw.url, width: width as number, height: height as number }];
  });
  return images.sort((left, right) => left.width - right.width);
}

function musicBrainzArtworkCandidates(track: Track, provenance: Record<string, unknown>): ArtworkImage[] {
  if (provenance.catalogProvider !== "musicbrainz") return [];
  const releaseId = provenance.releaseId;
  const groupId = provenance.releaseGroupId;
  const rawAlternates = Array.isArray(provenance.artworkAlternates) ? provenance.artworkAlternates : [];
  const candidates: ArtworkImage[] = [];
  for (const raw of rawAlternates) {
    if (typeof raw !== "string" || raw.length > 2048) continue;
    let url: URL;
    try { url = new URL(raw); } catch { continue; }
    if (url.protocol !== "https:" || url.hostname !== "coverartarchive.org" || url.username || url.password || url.port) continue;
    const match = url.pathname.match(/^\/(release|release-group)\/([0-9a-f-]{36})\/front-250$/i);
    if (!match) continue;
    const [, entity, id] = match;
    if (entity === "release" && (!validMbid(releaseId) || id.toLowerCase() !== releaseId.toLowerCase())) continue;
    if (entity === "release-group" && (!validMbid(groupId) || id.toLowerCase() !== groupId.toLowerCase())) continue;
    candidates.push({ url: `${url.origin}${url.pathname.replace(/front-250$/, "front-500")}`, width: 500, height: 500 });
    candidates.push({ url: `${url.origin}${url.pathname.replace(/front-250$/, "front-1200")}`, width: 1200, height: 1200 });
  }
  // Keep edition artwork ahead of group art even if upstream order changes.
  const edition = candidates.filter(candidate => candidate.url.includes(`/release/${String(releaseId)}/`));
  const group = candidates.filter(candidate => candidate.url.includes(`/release-group/${String(groupId)}/`));
  const original = track.artworkUrl;
  return [...edition, ...group].filter(candidate => candidate.url !== original);
}

/** Resize only the already-selected YouTube image asset; never search for replacement art. */
function youtubeArtworkCandidates(track: Track): ArtworkImage[] {
  if (track.provider !== "youtube" || typeof track.artworkUrl !== "string" || track.artworkUrl.length > 2048) return [];
  let url: URL;
  try { url = new URL(track.artworkUrl); } catch { return []; }
  if (url.protocol !== "https:" || !["yt3.googleusercontent.com", "yt3.ggpht.com"].includes(url.hostname)
    || url.username || url.password || url.port || url.search || url.hash) return [];
  const match = url.pathname.match(/^(\/[A-Za-z0-9_-]+)=w([0-9]{1,5})-h([0-9]{1,5})((?:-[A-Za-z0-9]+)*)$/);
  if (!match) return [];
  const [, asset, width, height, options] = match;
  return [600, 1200].filter(size => size > Math.max(Number(width), Number(height)))
    .map(size => ({url:`${url.origin}${asset}=w${size}-h${size}${options}`,width:size,height:size}));
}

/** Alternatives retain this track's catalog identity or its exact selected image asset. */
export function fullscreenArtworkUpgradeCandidates(track: Track): ArtworkImage[] {
  const provenance = isRecord(track.provenance) ? track.provenance : {};
  const catalog = isRecord(provenance.catalog) ? provenance.catalog : null;
  if (track.provider === "spotify" || catalog?.provider === "spotify") return spotifyImageCandidates(track, provenance);
  const youtube = youtubeArtworkCandidates(track);
  return youtube.length ? youtube : musicBrainzArtworkCandidates(track, provenance);
}
