/** Animate only the seek control, keeping backend updates and app state unchanged. */
export function smoothPlaybackPosition(read: () => { position: number; duration: number; playing: boolean; identity: unknown }) {
  let value = $state(0);
  $effect(() => {
    const { position, duration, playing, identity } = read();
    void identity;
    const anchor = Number.isFinite(position) ? Math.max(0, Math.min(duration || 0, position)) : 0;
    value = anchor;
    if (!playing || duration <= 0 || typeof requestAnimationFrame !== "function") return;
    const started = performance.now();
    let frame = 0;
    const advance = (now: number) => {
      // Stop extrapolating when reports stop arriving; never run into the next song.
      value = Math.min(duration, anchor + Math.min(1, Math.max(0, now - started) / 1000));
      frame = now - started < 1000 ? requestAnimationFrame(advance) : 0;
    };
    frame = requestAnimationFrame(advance);
    return () => cancelAnimationFrame(frame);
  });
  return { get value() { return value; } };
}
