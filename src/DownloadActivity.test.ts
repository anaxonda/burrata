// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client entry has no public declaration.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import DownloadActivity from "./DownloadActivity.svelte";

let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; });

const baseProps = {
  jobs: [], onClose: vi.fn(), onRefresh: vi.fn(), onCancel: vi.fn(), onRetry: vi.fn(), onShowFile: vi.fn(),
};

describe("DownloadActivity navigation", () => {
  it("only offers View all when the parent supplies a navigation action", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(DownloadActivity, { target, props: baseProps });
    cleanup = () => unmount(instance);
    await tick();
    expect(target.querySelector('[aria-label="View all downloads"]')).toBeNull();
    unmount(instance); cleanup = undefined; target.innerHTML = "";

    const onViewAll = vi.fn();
    const navigable = mount(DownloadActivity, { target, props: { ...baseProps, onViewAll } });
    cleanup = () => unmount(navigable);
    await tick();
    target.querySelector<HTMLButtonElement>('[aria-label="View all downloads"]')!.click();
    expect(onViewAll).toHaveBeenCalledOnce();
  });
});
