// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import AccountPlaylistSettings from "./AccountPlaylistSettings.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }

describe("AccountPlaylistSettings", () => {
  it("keeps each provider off by default and changes them independently", async () => {
    const onChange = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(AccountPlaylistSettings, { target, props: { onChange } });
    destroy = () => unmount(instance);
    await settle();
    const spotify = target.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    const youtube = target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')[1];
    expect(spotify.checked).toBe(false);
    expect(youtube.checked).toBe(false);

    spotify.click(); await settle();
    expect(onChange).toHaveBeenLastCalledWith({ spotify: true, youtube: false });

    destroy?.(); destroy = undefined; target.innerHTML = ""; onChange.mockClear();
    const youtubeInstance = mount(AccountPlaylistSettings, { target, props: { value: { spotify: true, youtube: false }, onChange } });
    destroy = () => unmount(youtubeInstance);
    await settle();
    target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')[1].click(); await settle();
    expect(onChange).toHaveBeenLastCalledWith({ spotify: true, youtube: true });
  });
});
