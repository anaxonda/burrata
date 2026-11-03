/** Project a canonical radio-song listing ID into storage identity fields. */
export function radioSongTrackIdentity(
  id: string,
): { id: string; provider: "radio_song"; providerId: string } | null {
  const prefix = "radio_song:";
  if (!id.startsWith(prefix)) return null;

  const providerId = id.slice(prefix.length);
  if (!providerId.trim() || providerId.startsWith(prefix)) return null;

  return { id, provider: "radio_song", providerId };
}
