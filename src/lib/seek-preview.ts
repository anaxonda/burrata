export function formatSeekTime(seconds: number): string {
  const whole = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  const minutes = Math.floor(whole / 60);
  return minutes >= 60
    ? `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")}:${String(whole % 60).padStart(2, "0")}`
    : `${minutes}:${String(whole % 60).padStart(2, "0")}`;
}

export function seekPreviewTime(clientX: number, left: number, width: number, duration: number): number | null {
  if (![clientX, left, width, duration].every(Number.isFinite) || width <= 0 || duration <= 0) return null;
  return Math.max(0, Math.min(1, (clientX - left) / width)) * duration;
}
