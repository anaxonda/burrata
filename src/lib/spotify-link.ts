export type SpotifyLink = {
  kind: "track" | "album" | "artist" | "playlist";
  id: string;
  url: string;
};

export function parseSpotifyLink(value: string): SpotifyLink | null {
  const input = value.trim();
  const uri = input.match(/^spotify:(track|album|artist|playlist):([A-Za-z0-9]{22})$/);
  let match = uri;
  if (!match) {
    try {
      const url = new URL(input);
      if (url.protocol !== "https:" || url.hostname !== "open.spotify.com" || url.username || url.password || url.port) return null;
      match = url.pathname.match(/^\/(?:intl-[a-z]{2}(?:-[A-Z]{2})?\/)?(track|album|artist|playlist)\/([A-Za-z0-9]{22})\/?$/);
    } catch {
      return null;
    }
  }
  if (!match) return null;
  const kind = match[1] as SpotifyLink["kind"];
  const id = match[2]!;
  return { kind, id, url: `https://open.spotify.com/${kind}/${id}` };
}
