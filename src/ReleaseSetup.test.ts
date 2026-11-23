// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";

const { callMock, listenMock } = vi.hoisted(() => ({ callMock: vi.fn(), listenMock: vi.fn(async (_event: string, _callback: (event: { payload: unknown }) => void) => () => {}) }));
vi.mock("./lib/api", () => ({ call: callMock, desktop: () => true, listen: listenMock }));
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
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; callMock.mockReset(); listenMock.mockClear(); });
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
    expect(callMock).toHaveBeenCalledWith("set_radio_recognition", { enabled: false });
    expect(target.textContent).toContain("Radio recognition setting saved.");
    expect(changed).toHaveBeenCalledOnce();
  });

  it("loads YouTube support only for relevant categories", async () => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status") return youtube;
      throw new Error(`Unexpected command ${command}`);
    });
    render({ category: "playback" }); await settle();
    expect(callMock).not.toHaveBeenCalledWith("youtube_support_status");
  });

  it("restores radio recognition when saving fails", async () => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true, radioRecognition: true });
      if (command === "set_radio_recognition") throw new Error("failed");
    });
    const target = render({ category: "playback" }); await settle();
    const checkbox = target.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    checkbox.click(); await settle();
    expect(checkbox.checked).toBe(true);
    expect(target.querySelector('[role="alert"]')?.textContent).toContain("failed");
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

  it("offers one primary stable update and groups optional tools under Advanced", async () => {
    callMock.mockImplementation(async (command: string) => command === "get_release_setup"
      ? setup({ setupComplete: true })
      : { ...youtube, provider: { ...youtube.provider, available: false } });
    const target = render({ category: "advanced" }); await settle();
    const advanced = [...target.querySelectorAll("details")].find(item => item.querySelector("summary")?.textContent === "Advanced options")!;
    expect(advanced.open).toBe(false);
    expect(target.textContent).not.toContain("needs attention");
    expect(target.textContent).toContain(`yt-dlp ${youtube.activeVersion} · Included with Burrata`);
    expect(target.textContent).toContain("Installs a newer stable release when available.");
    expect(button(target, "Update yt-dlp").closest("details")).toBeNull();
    expect(advanced.contains(button(target, "Install latest nightly build"))).toBe(true);
    expect(advanced.contains(button(target, "Check YouTube audio access"))).toBe(true);
    expect(callMock.mock.calls.some(([command]) => command === "youtube_support_diagnose" || command === "youtube_support_update")).toBe(false);
  });

  it.each([
    ["managed", "Installed version: yt-dlp 2026.10.26", "recorded during installation"],
    ["bundled", "Installed version: Unknown", "couldn’t verify the yt-dlp version"],
  ])("labels the installed version when the %s executable probe fails", async (activeSource, label, note) => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status") {
        return { ...youtube, activeSource, activeVersion: null, managedVersion: "2026.10.26" };
      }
      if (command === "youtube_support_check") return { version: "2026.11.21.232659", available: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render({ category: "advanced" }); await settle();
    expect(target.querySelector(".tool-version")?.textContent).toContain(label);
    expect(target.textContent).toContain(note);
  });

  it.each([youtube.activeVersion, "2026.10.26"])("updates the stable yt-dlp channel in one click (%s)", async (activeVersion) => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status") return youtube;
      if (command === "youtube_support_update") return activeVersion === youtube.activeVersion ? youtube : { ...youtube, activeVersion, activeSource: "managed" };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render({ category: "advanced" }); await settle();
    button(target, "Update yt-dlp").click(); await settle();
    expect(callMock).toHaveBeenCalledWith("youtube_support_update", { channel: "stable", operationId: expect.any(String) });
    expect(target.textContent).toContain(activeVersion === youtube.activeVersion
      ? "Your yt-dlp is already as recent as the latest stable release."
      : `Installed yt-dlp ${activeVersion} (stable release).`);
  });

  it("opts into the latest nightly and can restore the previously active version", async () => {
    const activeVersion = "2026.11.20.232656";
    let finishUpdate!: (value: unknown) => void;
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status" || command === "youtube_support_rollback") return youtube;
      if (command === "youtube_support_update") return new Promise(resolve => { finishUpdate = resolve; });
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render({ category: "advanced" }); await settle();
    const nightly = button(target, "Install latest nightly build");
    nightly.closest("details")!.open = true;
    nightly.click(); await settle();
    expect(callMock).toHaveBeenCalledWith("youtube_support_update", { channel: "nightly", operationId: expect.any(String) });
    expect(button(target, "Update yt-dlp").disabled).toBe(true);
    expect(target.textContent).toContain("Installing the latest yt-dlp nightly…");
    finishUpdate({ ...youtube, activeVersion, managedVersion: activeVersion, activeSource: "managed" });
    await settle();
    expect(target.textContent).toContain(`yt-dlp ${activeVersion} · Installed update`);
    expect(target.textContent).toContain(`Installed yt-dlp ${activeVersion} (nightly build).`);
    button(target, "Restore previous version").click(); await settle();
    expect(callMock).toHaveBeenCalledWith("youtube_support_rollback");
    expect(target.textContent).toContain(`Restored yt-dlp ${youtube.activeVersion}.`);
    expect(target.textContent).toContain("Included with Burrata");
  });

  it("shows an update failure while keeping the active version visible", async () => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status") return youtube;
      if (command === "youtube_support_update") throw new Error("Compatibility probe failed");
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render({ category: "advanced" }); await settle();
    button(target, "Update yt-dlp").click(); await settle();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain("Compatibility probe failed");
    expect(target.textContent).toContain(`yt-dlp ${youtube.activeVersion} · Included with Burrata`);
    expect(button(target, "Update yt-dlp").disabled).toBe(false);
  });

  it("shows available stable releases and scoped byte progress through verification", async () => {
    const activeVersion = "2026.10.26";
    const stop = vi.fn();
    let operationId = "";
    let finishUpdate!: (value: unknown) => void;
    let onProgress!: (event: { payload: unknown }) => void;
    listenMock.mockImplementationOnce(async (_event, callback) => {
      onProgress = callback;
      return stop;
    });
    callMock.mockImplementation(async (command: string, args?: { operationId: string }) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status") return youtube;
      if (command === "youtube_support_check") return { version: activeVersion, available: !operationId };
      if (command === "youtube_support_update") {
        operationId = args!.operationId;
        return new Promise(resolve => { finishUpdate = resolve; });
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render({ category: "advanced" }); await settle();
    expect(target.textContent).toContain(`yt-dlp ${activeVersion} is available (stable release).`);
    button(target, "Update yt-dlp").click(); await settle();
    const event = (op: string, progress: unknown) => onProgress({ payload: { operationId: op, progress } });
    event("old deferred operation", { stage: "downloading", receivedBytes: 50, totalBytes: 100 });
    await settle();
    expect(target.querySelector("progress")).toBeNull();
    event(operationId, { stage: "downloading", receivedBytes: 50, totalBytes: 100 });
    await settle();
    expect(target.querySelector("progress")?.value).toBe(50);
    expect(target.textContent).toContain("50%");
    event(operationId, { stage: "downloading", receivedBytes: 1_048_576, totalBytes: null });
    await settle();
    expect(target.querySelector("progress")?.hasAttribute("value")).toBe(false);
    expect(target.textContent).toContain("1.0 MiB downloaded");
    event(operationId, { stage: "verifying" });
    await settle();
    expect(target.textContent).toContain("Verifying the download");
    finishUpdate({ ...youtube, activeVersion, activeSource: "managed" });
    await settle();
    expect(stop).toHaveBeenCalledOnce();
    expect(target.textContent).toContain(`Latest stable release is ${activeVersion}.`);
  });

  it("keeps system or custom yt-dlp under its owner's control", async () => {
    callMock.mockImplementation(async (command: string) => command === "get_release_setup"
      ? { ...setup({ setupComplete: true }), usesManagedYtDlp: false }
      : youtube);
    const target = render({ category: "advanced" }); await settle();
    expect(target.textContent).toContain("Update your configured yt-dlp outside Burrata.");
    expect(button(target, "Update yt-dlp")).toBeUndefined();
    expect(button(target, "Install latest nightly build")).toBeUndefined();
  });

  it("keeps audio diagnosis independent of updates", async () => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_release_setup") return setup({ setupComplete: true });
      if (command === "youtube_support_status") return youtube;
      if (command === "youtube_support_diagnose") return { ok: false, category: "http403", message: "Public audio could not be reached." };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(); await settle();
    button(target, "Check YouTube audio access").click(); await settle();
    expect(callMock).toHaveBeenCalledWith("youtube_support_diagnose", {});
    expect(target.textContent).toContain("Try the check again. If it still fails, update yt-dlp.");
    expect(target.textContent).toContain("Public audio could not be reached.");
    expect(callMock.mock.calls.some(([command]) => command === "youtube_support_update")).toBe(false);
  });
});
