// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
// @ts-expect-error Use Svelte's reactive props object to simulate a selected-track change.
import { proxy } from "../node_modules/svelte/src/internal/client/proxy.js";
import SpotifyRecordingIdentityAction from "./SpotifyRecordingIdentityAction.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, onRetryPreview = vi.fn(), albumIdHint?: string) {
  const target = document.createElement("div"); document.body.append(target);
  const props = proxy({ trackId: "1234567890123456789012", albumIdHint, call, onRetryPreview });
  const instance = mount(SpotifyRecordingIdentityAction, { target, props });
  destroy = () => unmount(instance);
  return { target, props, onRetryPreview };
}

describe("SpotifyRecordingIdentityAction", () => {
  it("verifies only the exact Spotify ID and offers an explicit Preview retry", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "prepare_spotify_recording_crosswalk") return { verified: true, spotifyTrackId: args?.trackId, musicbrainzReleaseId: "release-id", spotifyPosition: 3 };
      return true;
    });
    const { target, onRetryPreview } = render(call); await settle();
    target.querySelector<HTMLButtonElement>('button:not(.retry)')!.click(); await settle();
    expect(call).toHaveBeenCalledWith("prepare_spotify_recording_crosswalk", { trackId: "1234567890123456789012", requestId: expect.stringMatching(/^[0-9a-f-]{36}$/i) });
    expect(target.textContent).toContain("Recording identity verified");
    target.querySelector<HTMLButtonElement>(".retry")!.click();
    expect(onRetryPreview).toHaveBeenCalledTimes(1);
  });

  it("passes only a valid existing album locator as a hint", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "prepare_spotify_recording_crosswalk") return { verified: true, spotifyTrackId: args?.trackId, musicbrainzReleaseId: "release-id", spotifyPosition: 3 };
      return true;
    });
    const { target } = render(call, vi.fn(), "abcdefghijklmnopqrstuv"); await settle();
    target.querySelector<HTMLButtonElement>('button:not(.retry)')!.click(); await settle();
    expect(call).toHaveBeenCalledWith("prepare_spotify_recording_crosswalk", {
      trackId: "1234567890123456789012", albumIdHint: "abcdefghijklmnopqrstuv", requestId: expect.stringMatching(/^[0-9a-f-]{36}$/i),
    });
  });

  it("cancels its owned request and ignores a late result after the selected track changes", async () => {
    const pending = deferred<unknown>();
    const call = vi.fn((command: string, _args?: Record<string, unknown>) => command === "cancel_remote_read" ? Promise.resolve(true) : pending.promise);
    const { target, props } = render(call); await settle();
    target.querySelector<HTMLButtonElement>('button:not(.retry)')!.click(); await settle();
    const requestId = call.mock.calls.find(([command]) => command === "prepare_spotify_recording_crosswalk")?.[1]?.requestId;
    props.trackId = "abcdefghijklmnopqrstuv"; await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId });
    pending.resolve({ verified: true, spotifyTrackId: "1234567890123456789012", musicbrainzReleaseId: "release-id", spotifyPosition: 3 });
    await settle();
    expect(target.textContent).not.toContain("Recording identity verified");
  });

  it("cancels the request when the action is unmounted", async () => {
    const pending = deferred<unknown>();
    const call = vi.fn((command: string, _args?: Record<string, unknown>) => command === "cancel_remote_read" ? Promise.resolve(true) : pending.promise);
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>('button:not(.retry)')!.click(); await settle();
    const requestId = call.mock.calls.find(([command]) => command === "prepare_spotify_recording_crosswalk")?.[1]?.requestId;
    const remove = destroy; destroy = undefined; remove?.(); await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId });
  });

  it("cancels and rejects a stale response when only the album locator changes", async () => {
    const pending = deferred<unknown>();
    const call = vi.fn((command: string, _args?: Record<string, unknown>) => command === "cancel_remote_read" ? Promise.resolve(true) : pending.promise);
    const { target, props } = render(call, vi.fn(), "abcdefghijklmnopqrstuv"); await settle();
    target.querySelector<HTMLButtonElement>('button:not(.retry)')!.click(); await settle();
    const requestId = call.mock.calls.find(([command]) => command === "prepare_spotify_recording_crosswalk")?.[1]?.requestId;
    props.albumIdHint = "zyxwvutsrqponmlkjihgfe"; await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId });
    pending.resolve({ verified: true, spotifyTrackId: "1234567890123456789012", musicbrainzReleaseId: "release-id", spotifyPosition: 3 });
    await settle();
    expect(target.textContent).not.toContain("Recording identity verified");
  });
});
