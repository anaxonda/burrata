// A playing flag alone cannot prove that an audio output is consuming frames.
export function hasPlaybackAdvanced(previous, current) {
  if (!previous || !current || previous.status !== 'playing' || current.status !== 'playing') return false;
  if (!Number.isFinite(previous.position) || !Number.isFinite(current.position)
      || previous.position < 0 || current.position < 0) return false;
  if (!Number.isInteger(previous.currentIndex) || !Number.isInteger(current.currentIndex)
      || previous.currentIndex < 0 || current.currentIndex < 0) return false;
  if (current.currentIndex === previous.currentIndex) return current.position > previous.position + 0.25;
  // Six-minute copies are sampled every 15 seconds, so at most one transition is expected.
  return current.currentIndex === previous.currentIndex + 1;
}
