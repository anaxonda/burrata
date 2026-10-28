export const PLAYBACK_CONTROLS_KEY = "burrata.playback-controls.v1";
export const ENGINE_SWITCH_CONTROLS_KEY = "burrata.engine-switch-controls.v1";

export interface PlaybackControls { volume: number; shuffle: boolean }

export function parsePlaybackControls(value: unknown): PlaybackControls | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const controls = value as Partial<PlaybackControls>;
  if (typeof controls.volume !== "number" || !Number.isFinite(controls.volume) || controls.volume < 0 || controls.volume > 1 || typeof controls.shuffle !== "boolean") return null;
  return { volume: controls.volume, shuffle: controls.shuffle };
}

type ReadableControlStorage = Pick<Storage, "getItem">;
type ConsumableControlStorage = Pick<Storage, "getItem" | "removeItem">;

export function readPlaybackControls(storage?: ReadableControlStorage): PlaybackControls | null {
  try {
    const target = storage ?? (typeof localStorage === "undefined" ? undefined : localStorage);
    if (!target) return null;
    const raw = target.getItem(PLAYBACK_CONTROLS_KEY);
    return raw == null ? null : parsePlaybackControls(JSON.parse(raw));
  } catch { return null; }
}

export function writePlaybackControls(storage: Storage, controls: PlaybackControls): void {
  if (!parsePlaybackControls(controls)) throw new Error("Invalid playback controls");
  storage.setItem(PLAYBACK_CONTROLS_KEY, JSON.stringify(controls));
}

/** One-shot engine-switch checkpoints take precedence, then durable user controls. */
export function consumeStartupPlaybackControls(storage?: ConsumableControlStorage): PlaybackControls | null {
  try {
    const target = storage ?? (typeof localStorage === "undefined" ? undefined : localStorage);
    if (!target) return null;
    const oneShotRaw = target.getItem(ENGINE_SWITCH_CONTROLS_KEY);
    const oneShot = oneShotRaw == null ? null : parsePlaybackControls(JSON.parse(oneShotRaw));
    target.removeItem(ENGINE_SWITCH_CONTROLS_KEY);
    return oneShot ?? readPlaybackControls(target);
  } catch { return readPlaybackControls(storage); }
}

export function persistPlaybackControlsWhenOwned(
  storage: Storage,
  controls: PlaybackControls,
  ownership: { desktop: boolean; hydrated: boolean; engineOwned: boolean },
): boolean {
  if (!ownership.desktop || !ownership.hydrated || !ownership.engineOwned || !parsePlaybackControls(controls)) return false;
  try { writePlaybackControls(storage, controls); return true; }
  catch { return false; }
}
