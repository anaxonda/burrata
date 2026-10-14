// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import TrackMenu from "./TrackMenu.svelte";
import type { Track } from "./lib/types";

const track: Track = { id: "t1", provider: "library", providerId: "t1", sourceUrl: "", artist: "Artist", title: "Song", favorite: false, localPath: "/music/song.flac", provenance: {} };
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });

describe("TrackMenu upgrade action", () => {
  it("shows downloaded availability and invokes the upgrade callback for any track", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const onUpgrade = vi.fn(), onClose = vi.fn(), noop = vi.fn();
    const instance = mount(TrackMenu, { target: document.body, props: {
      track: { ...track, localPath: null, localAvailability: "available" }, onUpgrade, onClose, onNext: noop, onQueue: noop,
      onPlaylist: noop, onFavorite: noop, onDownload: noop, onDetails: noop, onSelect: noop,
    } as never });
    destroy = () => unmount(instance);
    await tick();
    expect(document.body.textContent).toContain("Downloaded");
    const upgrade = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Find higher-quality download"));
    expect(upgrade).toBeTruthy();
    upgrade!.click(); await tick();
    expect(onUpgrade).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });
});
