/** Keep captured page titles in storage; remove known site boilerplate for display. */
export function playlistTitle(title: string, provider?: string): string {
  if (provider === "bandcamp") return title.replace(/\s+music and merch on Bandcamp\s*$/i, "").trim() || title;
  if (provider === "beatport") return title.replace(/\s*\|\s*Download & Stream.*$/i, "").trim() || title;
  return title;
}
