// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";

const { callMock } = vi.hoisted(() => ({ callMock: vi.fn() }));
vi.mock("./lib/api", () => ({ call: callMock, desktop: () => true }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ReleaseSetup from "./ReleaseSetup.svelte";

const setup = (overrides: Record<string, unknown> = {}) => ({
  setupComplete: false, musicDirectory: null, radioRecognition: false, mcpCommand: ["burrata", "--mcp"],
  browser: { browsers: [{ browser: "chrome", installed: false }, { browser: "firefox", installed: true }] },
  toolPaths: [{ name: "Extension folder", path: "/app/extension", origin: "Bundled with Burrata" }], ...overrides,
});
const youtube = { bundledVersion: "2026.01", managedVersion: null, activeVersion: "2026.01", activeSource: "bundled", updateAvailable: null, recommendedUpdate: "Not checked", advancedUpdate: "Official nightly", provider: { version: "2.0.2", available: true, helperRunning: false, message: "Anonymous provider is available." } };
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; callMock.mockReset(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(ReleaseSetup, { target, props }); destroy = () => unmount(instance); return target;
}
function button(target: HTMLElement, text: string) { return [...target.querySelectorAll("button")].find((item) => item.textContent?.trim() === text)!; }

describe("ReleaseSetup", () => {
  it("loads real setup state, chooses a folder, and saves onboarding choices", async () => {
    const changed = vi.fn(), complete = vi.fn();
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup();
      if (command === "youtube_support_status") return youtube;
      if (command === "choose_music_directory") return "/Music/Burrata";
      if (command === "complete_release_setup") return undefined;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render({ onboarding: true, onSettingsChanged: changed, onComplete: complete }); await settle();
    expect(target.textContent).toContain("Choose your music folder");
    expect(target.textContent).toContain("does not read cookies or logins");
    button(target, "Choose folder").click(); await settle();
    expect(target.textContent).toContain("/Music/Burrata");
    const checkbox = target.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    checkbox.click(); await settle();
    button(target, "Complete setup").click(); await settle();
    expect(callMock).toHaveBeenCalledWith("complete_release_setup", { radioRecognition: true });
    expect(changed).toHaveBeenCalledTimes(2); expect(complete).toHaveBeenCalledOnce();
    expect(target.textContent).toContain("Setup saved.");
  });

  it("saves radio recognition through the standalone setting command", async () => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true, radioRecognition: true });
      if (command === "youtube_support_status") return youtube;
      if (command === "set_radio_recognition") return undefined;
      throw new Error(`Unexpected command ${command}`);
    });
    const changed = vi.fn();
    const target = render({ onSettingsChanged: changed }); await settle();
    const checkbox = target.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    checkbox.click(); await settle();
    button(target, "Save setting").click(); await settle();
    expect(callMock).toHaveBeenCalledWith("set_radio_recognition", { enabled: false });
    expect(target.textContent).toContain("Radio recognition setting saved.");
    expect(changed).toHaveBeenCalledOnce();
  });

  it("registers a selected browser host and keeps errors visible", async () => {
    const changed = vi.fn();
    let hostInstalled = false;
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true, browser: { browsers: [{ browser: "chrome", installed: hostInstalled }] } });
      if (command === "youtube_support_status") return youtube;
      if (command === "register_browser_host") { hostInstalled = true; return { browsers: [{ browser: "chrome", installed: true }] }; }
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render({ onSettingsChanged: changed }); await settle();
    button(target, "Connect browser companion").click(); await settle();
    expect(callMock).toHaveBeenCalledWith("register_browser_host");
    expect(callMock).toHaveBeenCalledWith("get_release_setup");
    expect(target.textContent).toContain("Browser host registered");
    expect(changed).toHaveBeenCalledOnce();
  });

  it("diagnoses before offering repair and requires a separate explicit update click", async () => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status") return youtube;
      if (command === "youtube_support_diagnose") return { ok: false, category: "http403", ytDlpVersion: "2026.01", providerAvailable: true, audioAccessible: false, message: "Public audio could not be reached." };
      if (command === "youtube_support_check") return { channel: "recommended", version: null, publishedAt: null, available: false, message: "No recommended update is available." };
      if (command === "youtube_support_update") throw new Error("Must not update automatically");
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(); await settle();
    button(target, "Diagnose YouTube support").click(); await settle();
    expect(callMock).toHaveBeenCalledWith("youtube_support_diagnose", {});
    expect(callMock).toHaveBeenCalledWith("youtube_support_check", { channel: "recommended" });
    expect(target.textContent).toContain("Public audio could not be reached.");
    expect(target.textContent).toContain("No recommended update is available.");
    expect(callMock).not.toHaveBeenCalledWith("youtube_support_update", expect.anything());
  });
});
