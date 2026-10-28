// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("./lib/api", () => ({ call: mocks.call }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RestoreWorkReview from "./RestoreWorkReview.svelte";

const pausedPolicy = {
  reviewRequired: true,
  snapshotId: "snapshot-test",
  reviewedAt: null,
  warning: null,
  categories: [
    { kind: "metadata_retag_analysis", paused: true, pendingCount: 3, writesAudioTags: true, queriesProviders: false },
    { kind: "asset_autotag", paused: true, pendingCount: 2, writesAudioTags: true, queriesProviders: true },
    { kind: "background_source_preparation", paused: true, pendingCount: 1, writesAudioTags: false, queriesProviders: true },
    { kind: "background_stream_media_analysis", paused: true, pendingCount: 0, writesAudioTags: false, queriesProviders: true },
  ],
};

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.clearAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }

describe("RestoreWorkReview", () => {
  it("shows pending categories and persists only explicit reviewed choices", async () => {
    const call = mocks.call.mockImplementation(async (command: string) => {
      if (command === "get_restore_work_policy") return pausedPolicy;
      if (command === "review_restored_automatic_work") return {
        ...pausedPolicy,
        reviewRequired: false,
        reviewedAt: "2026-10-25T12:00:00Z",
        categories: pausedPolicy.categories.map((category, index) => ({ ...category, paused: index !== 0 && index !== 3 })),
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const onReviewed = vi.fn();
    const instance = mount(RestoreWorkReview, { target: document.body, props: { onReviewed } });
    destroy = () => unmount(instance);
    await settle();

    expect(document.body.textContent).toContain("3 pending");
    expect(document.body.textContent).toContain("2 pending");
    expect(document.body.textContent).toContain("1 pending");
    expect(document.body.textContent).toContain("On playback");
    expect(document.body.textContent).toContain("does not save audio or change library metadata");
    expect(call).toHaveBeenCalledTimes(1);
    expect(call).toHaveBeenCalledWith("get_restore_work_policy");
    expect(onReviewed).toHaveBeenCalledWith(pausedPolicy);

    const checkboxes = [...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    expect(checkboxes.every((checkbox) => !checkbox.checked)).toBe(true);
    checkboxes[0].click();
    checkboxes[3].click();
    await settle();
    expect(call).toHaveBeenCalledTimes(1);

    document.querySelector<HTMLButtonElement>("button")!.click();
    await settle();
    expect(call).toHaveBeenNthCalledWith(2, "review_restored_automatic_work", {
      resume: {
        metadataRetagAnalysis: true,
        assetAutotag: false,
        backgroundSourcePreparation: false,
        backgroundStreamMediaAnalysis: true,
      },
    });
    expect(document.body.textContent).toContain("Your selected automatic work was resumed");
    expect(onReviewed).toHaveBeenCalledTimes(2);
    expect(onReviewed.mock.calls[1][0].reviewRequired).toBe(false);
    expect(onReviewed.mock.calls[1][0].categories.find((category: { kind: string }) => category.kind === "background_stream_media_analysis").paused).toBe(false);
  });

  it("preserves older reviewed choices when the new stream category remains paused", async () => {
    const mixedPolicy = {
      ...pausedPolicy,
      reviewRequired: false,
      reviewedAt: "2026-10-25T12:00:00Z",
      categories: pausedPolicy.categories.map((category) => ({
        ...category,
        paused: category.kind === "asset_autotag" || category.kind === "background_stream_media_analysis",
      })),
    };
    const call = mocks.call.mockImplementation(async (command: string) => {
      if (command === "get_restore_work_policy") return mixedPolicy;
      if (command === "review_restored_automatic_work") return mixedPolicy;
      throw new Error(`Unexpected command ${command}`);
    });
    const instance = mount(RestoreWorkReview, { target: document.body });
    destroy = () => unmount(instance);
    await settle();

    const checkboxes = [...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    expect(checkboxes.map((checkbox) => checkbox.checked)).toEqual([true, false, true, false]);
    checkboxes[3].click();
    await settle();
    document.querySelector<HTMLButtonElement>("button")!.click();
    await settle();

    expect(call).toHaveBeenNthCalledWith(2, "review_restored_automatic_work", {
      resume: {
        metadataRetagAnalysis: true,
        assetAutotag: false,
        backgroundSourcePreparation: true,
        backgroundStreamMediaAnalysis: true,
      },
    });
  });
});
