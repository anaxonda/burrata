import { describe, expect, it, vi } from "vitest";
import { consumeStartupPlaybackControls, persistPlaybackControlsWhenOwned, readPlaybackControls } from "./playback-controls";

describe("persistent playback controls", () => {
  it("loads durable controls on a second cold start after the restore marker is gone", () => {
    const values = new Map([["burrata.playback-controls.v1", JSON.stringify({ volume: 0, shuffle: true })]]);
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
      removeItem: (key: string) => { values.delete(key); },
    } as unknown as Storage;
    expect(consumeStartupPlaybackControls(storage)).toEqual({ volume: 0, shuffle: true });
    expect(readPlaybackControls(storage)).toEqual({ volume: 0, shuffle: true });
  });

  it("keeps one-shot engine-switch controls first and persists durable values only after hydration and engine ownership", () => {
    const values = new Map([["burrata.playback-controls.v1", JSON.stringify({ volume: 0.4, shuffle: false })], ["burrata.engine-switch-controls.v1", JSON.stringify({ volume: 0.7, shuffle: true })]]);
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
      removeItem: (key: string) => { values.delete(key); },
    } as unknown as Storage;
    expect(consumeStartupPlaybackControls(storage)).toEqual({ volume: 0.7, shuffle: true });
    const save = vi.spyOn(storage, "setItem");
    expect(persistPlaybackControlsWhenOwned(storage, { volume: 0.25, shuffle: false }, { desktop: true, hydrated: false, engineOwned: true })).toBe(false);
    expect(persistPlaybackControlsWhenOwned(storage, { volume: 0.25, shuffle: false }, { desktop: true, hydrated: true, engineOwned: false })).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(persistPlaybackControlsWhenOwned(storage, { volume: 0.25, shuffle: false }, { desktop: true, hydrated: true, engineOwned: true })).toBe(true);
    expect(readPlaybackControls(storage)).toEqual({ volume: 0.25, shuffle: false });
  });
});
