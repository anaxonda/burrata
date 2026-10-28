// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RestoredPlaylistSyncSettings from "./RestoredPlaylistSyncSettings.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }

describe("RestoredPlaylistSyncSettings", () => {
  it("keeps restored sync paused until the reviewed destination is explicitly confirmed", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "preview_restored_playlist_sync") return {
        reviewToken: "review-abc",
        currentMirrorDirectory: "/home/user/Music/Playlists",
        sourceCount: 2,
        mirrorCount: 4,
        protectedCopyCount: 1,
        sectionMirrorRelinks: [
          { playlistName: "Weekend Mix", sectionName: "Warmup", relativePath: "Weekend Mix/01 Warmup.m3u8" },
          { playlistName: "Weekend Mix", sectionName: "Late Set", relativePath: "Weekend Mix/02 Late Set.m3u8" },
        ],
      };
      if (command === "sync_playlists") return { warnings: [] };
      return {};
    });
    const instance = mount(RestoredPlaylistSyncSettings, { target: document.body, props: { call, desktop: true } as never });
    destroy = () => unmount(instance);
    await settle();

    expect(document.body.textContent).toContain("/home/user/Music/Playlists");
    expect(document.body.textContent).toContain("Saved playlist sources");
    expect(document.body.textContent).toContain("Saved playlist files");
    expect(document.body.textContent).toContain("Section files that match your backup");
    expect(document.body.textContent).toContain("Weekend Mix · Warmup");
    expect(document.body.textContent).toContain("Weekend Mix/01 Warmup.m3u8");
    expect(document.body.textContent).toContain("These files match your backup and can resume updating with your playlists.");
    expect(document.body.textContent.indexOf("Weekend Mix/01 Warmup.m3u8")).toBeLessThan(document.body.textContent.indexOf("I reviewed this folder"));
    const button = document.querySelector<HTMLButtonElement>("button")!;
    expect(button.disabled).toBe(true);
    expect(call).toHaveBeenCalledTimes(1);

    document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click();
    await settle();
    expect(button.disabled).toBe(false);
    button.click();
    await settle();
    expect(call).toHaveBeenNthCalledWith(2, "resume_reviewed_restored_playlist_sync", { reviewToken: "review-abc" });
    expect(call).toHaveBeenNthCalledWith(3, "sync_playlists");
    expect(document.body.textContent).toContain("Playlist files will update using this device’s current music folder.");
  });

  it("requires a fresh review when the reviewed folder state changes", async () => {
    let previews = 0;
    const call = vi.fn(async (command: string) => {
      if (command === "preview_restored_playlist_sync") {
        previews++;
        return {
          reviewToken: `review-${previews}`,
          currentMirrorDirectory: "/home/user/Music/Playlists",
          sourceCount: 1,
          mirrorCount: 1,
          protectedCopyCount: 1,
          sectionMirrorRelinks: previews === 1 ? [
            { playlistName: "Mix", sectionName: "Old", relativePath: "Mix/old.m3u8" },
          ] : [
            { playlistName: "Mix", sectionName: "Updated", relativePath: "Mix/updated.m3u8" },
          ],
        };
      }
      if (command === "resume_reviewed_restored_playlist_sync") throw new Error("Folder changed; review it again before resuming");
      return {};
    });
    const instance = mount(RestoredPlaylistSyncSettings, { target: document.body, props: { call, desktop: true } as never });
    destroy = () => unmount(instance);
    await settle();

    document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>("button")!.click();
    await settle();

    expect(previews).toBe(2);
    expect(document.body.textContent).toContain("Mix · Updated");
    expect(document.body.textContent).toContain("The folder review changed. Check the updated details before resuming.");
    expect(document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.checked).toBe(false);
    expect(document.querySelector<HTMLButtonElement>("button")!.disabled).toBe(true);
  });

  it.each([
    "There is no restored playlist projection to review",
    "Restored playlist synchronization is already active",
  ])("does not show a restore error for an ordinary active profile: %s", async (reason) => {
    const call = vi.fn(async () => { throw new Error(reason); });
    const instance = mount(RestoredPlaylistSyncSettings, { target: document.body, props: { call, desktop: true } as never });
    destroy = () => unmount(instance);
    await settle();
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(document.querySelector("button")).toBeNull();
    expect(document.body.textContent?.trim()).toBe("");
  });

  it("surfaces preview failures without enabling resume", async () => {
    const call = vi.fn(async () => { throw new Error("folder is unavailable"); });
    const instance = mount(RestoredPlaylistSyncSettings, { target: document.body, props: { call, desktop: true } as never });
    destroy = () => unmount(instance);
    await settle();

    expect(document.querySelector('[role="alert"]')?.textContent).toContain("folder is unavailable");
    expect(document.querySelector("button")).toBeNull();
  });
});
