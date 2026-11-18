import type { Track } from "./types";

type ArtworkImage = { url: string; width: number; height: number };
export type ArtworkUpgradeCandidate = ArtworkImage;

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function validMbid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}

function publicArtworkUrl(value: unknown): URL | null {
  if (typeof value !== "string" || value.length > 2048) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !url.username && !url.password && !url.port && !url.search && !url.hash ? url : null;
  } catch {
    return null;
  }
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
  for (const raw of new Set([...rawAlternates, track.artworkUrl])) {
    const url = publicArtworkUrl(raw);
    if (url?.hostname !== "coverartarchive.org") continue;
    const match = url.pathname.match(/^\/(release|release-group)\/([0-9a-f-]{36})\/front-(250|500)$/i);
    if (!match) continue;
    const [, entity, id, width] = match;
    if (entity === "release" && (!validMbid(releaseId) || id.toLowerCase() !== releaseId.toLowerCase())) continue;
    if (entity === "release-group" && (!validMbid(groupId) || id.toLowerCase() !== groupId.toLowerCase())) continue;
    if (width === "250") candidates.push({ url: `${url.origin}${url.pathname.replace(/front-250$/, "front-500")}`, width: 500, height: 500 });
    candidates.push({ url: `${url.origin}${url.pathname.replace(/front-(?:250|500)$/, "front-1200")}`, width: 1200, height: 1200 });
  }
  // Keep edition artwork ahead of group art even if upstream order changes.
  const edition = candidates.filter(candidate => candidate.url.includes(`/release/${String(releaseId)}/`));
  const group = candidates.filter(candidate => candidate.url.includes(`/release-group/${String(groupId)}/`));
  const original = track.artworkUrl;
  return [...edition, ...group].filter(candidate => candidate.url !== original);
}

function youtubeMusicArtworkCandidates(url: URL): ArtworkImage[] {
  const match = url.pathname.match(/^(\/[A-Za-z0-9_-]+)=w([0-9]{1,5})-h([0-9]{1,5})((?:-[A-Za-z0-9]+)*)$/);
  if (!match) return [];
  const [, asset, width, height, options] = match;
  return [600, 1200].filter(size => size > Math.max(Number(width), Number(height)))
    .map(size => ({url:`${url.origin}${asset}=w${size}-h${size}${options}`,width:size,height:size}));
}

function youtubeVideoArtworkCandidates(url: URL): ArtworkImage[] {
  const match = url.pathname.match(/^\/(vi|vi_webp)\/([A-Za-z0-9_-]{11})\/(default|mqdefault|hqdefault|sddefault)\.(jpg|webp)$/);
  if (!match) return [];
  const [, folder, id, size, extension] = match;
  if ((folder === "vi_webp" ? "webp" : "jpg") !== extension) return [];
  const base = `${url.origin}/${folder}/${id}/`;
  const maxres = { url: `${base}maxresdefault.${extension}`, width: 1280, height: 720 };
  return size === "sddefault" ? [maxres] : [
    { url: `${base}sddefault.${extension}`, width: 640, height: 480 },
    maxres,
  ];
}

/** Bandcamp's image suffix selects a size of the same cover asset. */
function bandcampArtworkCandidates(url: URL): ArtworkImage[] {
  const match = url.pathname.match(/^(\/img\/a[0-9]{1,20})_(2|3|5|7|16)\.jpg$/);
  if (!match) return [];

  const [, asset, suffix] = match;
  const larger = { url: `${url.origin}${asset}_10.jpg`, width: 1200, height: 1200 };
  return suffix === "5" || suffix === "16" ? [larger] : [
    { url: `${url.origin}${asset}_5.jpg`, width: 700, height: 700 },
    larger,
  ];
}

function soundCloudArtworkCandidates(url: URL): ArtworkImage[] {
  const match = url.pathname.match(/^(\/artworks-[a-zA-Z0-9_-]+)-(?:large|t([0-9]{1,4})x([0-9]{1,4}))\.(jpg|jpeg|png)$/);
  if (!match) return [];
  const [, asset, width, height, extension] = match;
  const originalSize = Math.max(Number(width || 100), Number(height || 100));
  return [500, 1080].filter(size => size > originalSize)
    .map(size => ({ url: `${url.origin}${asset}-t${size}x${size}.${extension}`, width: size, height: size }));
}

function storeArtworkCandidates(url: URL, path: RegExp, sizes: readonly number[]): ArtworkImage[] {
  const match = url.pathname.match(path);
  if (!match) return [];
  const [, prefix, width, height, asset] = match;
  const originalSize = Math.max(Number(width), Number(height));
  return sizes.filter(size => size > originalSize)
    .map(size => ({ url: `${url.origin}${prefix}${size}x${size}/${asset}`, width: size, height: size }));
}

function selectedArtworkCandidates(artworkUrl: Track["artworkUrl"]): ArtworkImage[] {
  const url = publicArtworkUrl(artworkUrl);
  if (!url) return [];
  const host = url.hostname;
  if (host === "yt3.googleusercontent.com" || host === "yt3.ggpht.com") return youtubeMusicArtworkCandidates(url);
  if (host === "i.ytimg.com" || host === "img.youtube.com") return youtubeVideoArtworkCandidates(url);
  if (/^f[1-4]\.bcbits\.com$/.test(host)) return bandcampArtworkCandidates(url);
  if (/^i[1-4]\.sndcdn\.com$/.test(host)) return soundCloudArtworkCandidates(url);
  if (host === "geo-media.beatport.com") {
    return storeArtworkCandidates(url, /^(\/image_size\/)([0-9]{1,4})x([0-9]{1,4})\/([a-zA-Z0-9-]{36}\.(?:jpg|jpeg|png))$/, [600, 1200]);
  }
  if (host === "geo-static.traxsource.com" || host === "geo-w-static.traxsource.com") {
    return storeArtworkCandidates(url, /^(\/scripts\/image\.php\/)([0-9]{1,4})x([0-9]{1,4})\/([a-fA-F0-9]{32}\.(?:jpg|jpeg|png))$/, [600, 1000]);
  }
  return [];
}

/** Alternatives retain this track's catalog identity or its exact selected image asset. */
export function fullscreenArtworkUpgradeCandidates(track: Track): ArtworkImage[] {
  const provenance = isRecord(track.provenance) ? track.provenance : {};
  const catalog = isRecord(provenance.catalog) ? provenance.catalog : null;
  if (track.provider === "spotify" || catalog?.provider === "spotify") return spotifyImageCandidates(track, provenance);
  const selected = selectedArtworkCandidates(track.artworkUrl);
  return selected.length ? selected : musicBrainzArtworkCandidates(track, provenance);
}
