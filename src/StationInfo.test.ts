// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
const { callMock } = vi.hoisted(() => ({ callMock: vi.fn() }));
vi.mock("./lib/api", () => ({ call: callMock, desktop: () => true }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import StationInfo from "./StationInfo.svelte";
import { SOURCE_OPENER } from "./lib/source-opening";
import type { Track } from "./lib/types";

let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; });

async function render(props: Record<string, unknown>, opener?: (track: Track, url: string) => Promise<void>) {
  const target = document.createElement("div"); document.body.append(target);
  const context = opener ? new Map([[SOURCE_OPENER, opener]]) : undefined;
  const instance = mount(StationInfo, { target, props: props as never, context });
  cleanup = () => unmount(instance);
  await tick();
  return target;
}

describe("StationInfo", () => {
  it("reveals safe station and stream links from the keyboard-operable disclosure", async () => {
    const target = await render({ stationName: "Radio Nova", stationUrl: "https://radio.example/home", streamUrl: "http://stream.example/live", compact: true });
    const summary = target.querySelector("summary")!;
    expect(summary.getAttribute("aria-label")).toBe("Station information for Radio Nova");
    expect(target.textContent).toContain("Radio Nova");
    summary.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await tick();
    expect(target.querySelector('a[href="https://radio.example/home"]')?.getAttribute("rel")).toBe("noopener noreferrer");
    expect(target.querySelector('a[href="http://stream.example/live"]')).not.toBeNull();
  });

  it("omits unsafe URLs and never renders embedded credentials", async () => {
    const target = await render({ stationName: "Radio Nova", stationUrl: "javascript:alert(1)", streamUrl: "https://user:secret@stream.example/live" });
    expect(target.querySelector("a")).toBeNull();
    expect(target.textContent).not.toContain("secret");
  });

  it("opens a disclosed URL through the configured source opener", async () => {
    const track = { id: "radio:test", title: "Test radio", artist: "Radio", favorite: false } as Track;
    const opener = vi.fn().mockResolvedValue(undefined);
    const target = await render({ stationName: "Radio Nova", stationUrl: "https://radio.example/home", track }, opener);
    target.querySelector("summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await tick();
    target.querySelector<HTMLAnchorElement>('a[href="https://radio.example/home"]')!.click();
    expect(opener).toHaveBeenCalledWith(track, "https://radio.example/home");
  });

  it("light-dismisses outside clicks and closes on Escape with focus returned to the station", async () => {
    const target = await render({ stationName: "Radio Nova", stationUrl: "https://radio.example/home" });
    const details = target.querySelector("details") as HTMLDetailsElement;
    const summary = target.querySelector("summary") as HTMLElement;
    summary.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await tick();
    expect(details.open).toBe(true);
    document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    await tick();
    expect(details.open).toBe(false);

    summary.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await tick();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick();
    expect(details.open).toBe(false);
    expect(document.activeElement).toBe(summary);
  });

  it("renders no disclosure for an empty station name", async () => {
    const target = await render({ stationName: "  ", stationUrl: "https://radio.example" });
    expect(target.querySelector("details")).toBeNull();
  });
});
