/** Selection changes do not stop playback; Space auditions a newly selected recording. */
export function playbackAction(
  selectedId: string | undefined,
  playingId: string | undefined,
  hasPlayableAudio: boolean,
): "start-selected" | "toggle-current" | "none" {
  if (selectedId && selectedId !== playingId) return "start-selected";
  if (hasPlayableAudio) return "toggle-current";
  return selectedId ? "start-selected" : "none";
}
