// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./lib/api", () => ({ call: vi.fn(), desktop: () => true }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ToolSetup from "./ToolSetup.svelte";
import type { Dependency, Settings } from "./lib/types";

const settings: Settings = {
  musicDirectory: "", autoPreview: false, previewDelayMs: 250, theme: "dark",
  ytDlpPath: "yt-dlp", ffmpegPath: "ffmpeg", ffprobePath: "ffprobe", jsRuntimePath: "node",
};
const dependencies: Dependency[] = [];
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { await tick(); }
function render(onChange: (value: Settings) => void) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(ToolSetup, { target, props: { settings, dependencies, onChange, onCheck: vi.fn(), onSave: vi.fn() } });
  destroy = () => unmount(instance); return target;
}

describe("Included tools summary", () => {
  it("shows unchecked tools and keeps advanced controls collapsed", async () => {
    const onCheck = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(ToolSetup, { target, props: { settings, dependencies: [], onChange: vi.fn(), onCheck, onSave: vi.fn() } });
    destroy = () => unmount(instance); await settle();
    expect(target.querySelector(".tool-summary")?.textContent).toContain("not been checked yet");
    expect(target.querySelector<HTMLDetailsElement>(".tool-details")?.open).toBe(false);
    expect(target.querySelector(".tool-details > summary")?.textContent).toContain("Tool details and custom locations");
    target.querySelector<HTMLButtonElement>(".setup-heading button")!.click();
    expect(onCheck).toHaveBeenCalledOnce();
  });

  it("distinguishes available, missing and unknown tools without showing versions in the summary", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const checked: Dependency[] = [
      { name: "yt-dlp", available: true, version: "2026.01", message: "" },
      { name: "FFmpeg", available: false, message: "not found" },
      { name: "ffprobe", available: false, message: "Unknown: check required" },
      { name: "JavaScript runtime", available: false, message: "not found" },
    ];
    const instance = mount(ToolSetup, { target, props: { settings, dependencies: checked, onChange: vi.fn(), onCheck: vi.fn(), onSave: vi.fn() } });
    destroy = () => unmount(instance); await settle();
    const summary = target.querySelector(".tool-summary")!.textContent!;
    expect(summary).toContain("Attention needed: audio conversion is unavailable");
    expect(summary).not.toContain("2026.01");
    expect(summary).not.toContain("JavaScript runtime");
    expect(target.querySelector(".tool-details")?.textContent).toContain("JavaScript runtime (optional)");
  });

  it("reports required tools available even when the optional runtime is missing", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const checked: Dependency[] = ["yt-dlp", "FFmpeg", "ffprobe"].map(name => ({ name, available: true, message: "" }));
    checked.push({ name: "JavaScript runtime", available: false, message: "not found" });
    const instance = mount(ToolSetup, { target, props: { settings, dependencies: checked, onChange: vi.fn(), onCheck: vi.fn(), onSave: vi.fn() } });
    destroy = () => unmount(instance); await settle();
    expect(target.querySelector(".tool-summary")?.textContent).toContain("Required audio tools are available");
  });
});

describe("ToolSetup executable sources", () => {
  it("treats the default node command as bundled or managed", async () => {
    const target = render(vi.fn()); await settle();
    const selector = target.querySelector<HTMLSelectElement>('[aria-label="JavaScript runtime executable source"]')!;
    expect(selector.value).toBe("bundled");
    expect(selector.parentElement?.querySelector("input")).toBeNull();
  });

  it("stores system and custom choices in the settings value", async () => {
    const onChange = vi.fn();
    const target = render(onChange); await settle();
    const selector = target.querySelector<HTMLSelectElement>('[aria-label="JavaScript runtime executable source"]')!;
    selector.value = "system"; selector.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(onChange).toHaveBeenLastCalledWith({ ...settings, jsRuntimePath: "system:node" });

    selector.value = "custom"; selector.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(onChange).toHaveBeenLastCalledWith({ ...settings, jsRuntimePath: "" });
  });

  it("selecting bundled mode writes the strict managed executable marker", async () => {
    const customSettings = { ...settings, jsRuntimePath: "/opt/deno/bin/deno" };
    const target = document.createElement("div"); document.body.append(target);
    const onChange = vi.fn();
    const instance = mount(ToolSetup, { target, props: { settings: customSettings, dependencies, onChange, onCheck: vi.fn(), onSave: vi.fn() } });
    destroy = () => unmount(instance); await settle();
    const selector = target.querySelector<HTMLSelectElement>('[aria-label="JavaScript runtime executable source"]')!;
    expect(selector.value).toBe("custom");
    expect(selector.parentElement?.querySelector("input")?.value).toBe("/opt/deno/bin/deno");
    selector.value = "bundled"; selector.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(onChange).toHaveBeenLastCalledWith({ ...customSettings, jsRuntimePath: "bundled:node" });
  });
});
