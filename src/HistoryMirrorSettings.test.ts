// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import HistoryMirrorSettings from "./HistoryMirrorSettings.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }

function mountSettings(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const instance = mount(HistoryMirrorSettings, { target: document.body, props: { call, desktop: true } as never });
  destroy = () => unmount(instance);
}

describe("HistoryMirrorSettings", () => {
  it("loads the all-history default and persists an explicitly disclosed recent scope", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_history_mirror_options") return { schemaVersion: 1, recentDays: null };
      if (command === "set_history_mirror_options") return { schemaVersion: 1, recentDays: 14 };
      return {};
    });
    mountSettings(call);
    await settle();
    expect(call).toHaveBeenCalledWith("get_history_mirror_options");
    expect(document.body.textContent).toContain("All retained history");
    const recent = document.querySelector<HTMLInputElement>('input[value="recent"]')!;
    recent.click(); await settle();
    const days = document.querySelector<HTMLInputElement>("#history-mirror-recent-days")!;
    days.value = "14"; days.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Save history options"))!.click();
    await settle();
    expect(call).toHaveBeenCalledWith("set_history_mirror_options", { options: { schemaVersion: 1, recentDays: 14 } });
    expect(call).toHaveBeenCalledWith("sync_playlists");
    expect(document.body.textContent).toContain("the last 14 days will be included when Listening history is mirrored");
    expect(document.body.textContent).not.toContain("The mirror includes the last 14 days");
  });

  it("reports saved options separately when the subsequent mirror sync fails", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_history_mirror_options") return { schemaVersion: 1, recentDays: null };
      if (command === "set_history_mirror_options") return { schemaVersion: 1, recentDays: null };
      if (command === "sync_playlists") throw new Error("mirror is disabled");
      return {};
    });
    mountSettings(call);
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Save history options"))!.click();
    await settle();
    expect(call).toHaveBeenCalledWith("set_history_mirror_options", { options: { schemaVersion: 1, recentDays: null } });
    expect(document.body.textContent).toContain("History options saved, but playlist sync failed: Error: mirror is disabled");
  });
});
