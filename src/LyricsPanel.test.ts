// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import LyricsPanel, { type CurrentTrackLyrics } from "./LyricsPanel.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); await new Promise(resolve => setTimeout(resolve, 0)); await tick(); }
const track = { id: "synthetic-song", provider: "youtube", providerId: "synthetic", sourceUrl: "https://youtube.test/watch?v=synthetic", title: "Synthetic Song", artist: "Synthetic Artist", album: "Synthetic Album", durationSeconds: 180, favorite: false, provenance: {} };
function render(result: CurrentTrackLyrics, absolutePosition = 0, extra: Record<string, unknown> = {}, trackValue: typeof track = track) {
  const target = document.createElement("div"); document.body.append(target);
  const loadLyrics = vi.fn(async () => result);
  const instance = mount(LyricsPanel, { target, props: { track: trackValue, absolutePosition, loadLyrics, ...extra } as never });
  destroy = () => unmount(instance);
  return { target, call: loadLyrics };
}

describe("LyricsPanel", () => {
  it("applies source timing offset without rewriting lyric timestamps or resetting it with manual controls", async () => {
    const result: CurrentTrackLyrics = {status:"synced",provider:"localLrc",sourceOffsetSeconds:2,
      lines:[{timeSeconds:1,text:"First synthetic line"},{timeSeconds:3,text:"Next synthetic line"}]};
    const {target}=render(result,1.5);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Next synthetic line");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics later by 0.5 seconds"]')!.click();await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click();await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Next synthetic line");
    expect(result.lines![1].timeSeconds).toBe(3);
  });

  it("opens requested fullscreen lyrics immediately and reports close without an extra show click", async () => {
    const close=vi.fn();const {call}=render({status:"unsynced",provider:"lrclib",plainLyrics:"Direct fullscreen lyrics"},0,{fullscreenRequested:true,onFullscreenClose:close});
    await settle();expect(call).toHaveBeenCalledOnce();
    const dialog=document.body.querySelector<HTMLElement>('[aria-label="Fullscreen lyrics"]')!;
    expect(dialog.textContent).toContain("Direct fullscreen lyrics");
    dialog.querySelector<HTMLButtonElement>('[aria-label="Exit fullscreen lyrics"]')!.click();await settle();
    expect(close).toHaveBeenCalledOnce();
  });

  it("maximizes lyrics without instructional clutter and exits with Escape", async () => {
    const { target } = render({status:"synced",provider:"lrclib",lines:[{timeSeconds:0,text:"Fullscreen line"}]});
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    expect(target.textContent).not.toContain("loaded only");
    expect(target.textContent).not.toContain("enable a provider");
    expect(target.querySelector('[aria-label="Translate lyrics"]')).not.toBeNull();
    target.querySelector<HTMLButtonElement>('[aria-label="Maximize lyrics"]')!.click();await settle();
    const dialog=document.body.querySelector<HTMLElement>('[role="dialog"][aria-label="Fullscreen lyrics"]')!;
    expect(dialog).not.toBeNull();expect(dialog.parentElement).toBe(document.body);
    expect(dialog.textContent).toContain("Fullscreen line");
    dialog.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true}));await settle();
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(target.querySelector('.lyrics-panel')).not.toBeNull();
  });

  it("fetches only after opt-in and highlights the line at the absolute track timestamp", async () => {
    const result: CurrentTrackLyrics = { status: "synced", provider: "lrclib", lines: [
      { timeSeconds: 6, text: "Synthetic first line" }, { timeSeconds: 22, text: "Synthetic second line" },
    ] };
    const { target, call } = render(result, 10); await settle();
    expect(call).not.toHaveBeenCalled();
    target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(call).toHaveBeenCalledWith();
    expect(target.querySelectorAll(".lyrics-scroll p")).toHaveLength(2);
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic first line");
    expect(target.querySelectorAll(".lyrics-scroll p")[1]?.getAttribute("aria-current")).toBeNull();
  });

  it("applies a bounded local half-second timing correction without changing source timestamps", async () => {
    const lines = [{ timeSeconds: 6, text: "Synthetic first line" }, { timeSeconds: 10.25, text: "Synthetic next line" }];
    const { target } = render({ status: "synced", provider: "lrclib", lines }, 10);
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic first line");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.5 seconds"]')!.click(); await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic next line");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("+0.5 s");
    expect(lines[1].timeSeconds).toBe(10.25);
    target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click(); await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic first line");
    for (let i = 0; i < 61; i++) target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.5 seconds"]')!.click();
    await settle();
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("+30.0 s");
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.5 seconds"]')?.disabled).toBe(true);
  });

  it("adds an identified-audio anchor to manual timing and clears both with Reset", async () => {
    const syncLyrics = vi.fn(async () => ({ offsetSeconds: 2 }));
    const { target } = render({ status: "synced", provider: "lrclib", lines: [
      { timeSeconds: 6, text: "Synthetic first line" }, { timeSeconds: 12, text: "Synthetic next line" },
    ] }, 10, { syncLyrics, syncScope: "generation-1:seek-2" });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(target.querySelector(".lyrics-sync")?.textContent).toContain("a short sample from current native playback is sent");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.5 seconds"]')!.click();
    target.querySelector<HTMLButtonElement>(".lyrics-sync button")!.click(); await settle();
    expect(syncLyrics).toHaveBeenCalledTimes(1);
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic next line");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("Manual offset +0.5 s");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("Audio anchor +2.0 s");
    target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click(); await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic first line");
    expect(target.querySelector(".lyrics-timing")?.textContent).not.toContain("Audio anchor");
  });

  it("does not claim sync if identification has no timing anchor", async () => {
    const { target } = render({ status: "synced", provider: "lrclib", lines: [{ timeSeconds: 1, text: "Synthetic line" }] }, 5, {
      syncLyrics: async () => null, syncScope: "generation-1:seek-2",
    });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-sync button")!.click(); await settle();
    expect(target.querySelector(".lyrics-sync")?.textContent).toContain("No matching timing anchor was available");
    expect(target.querySelector(".lyrics-timing")?.textContent).not.toContain("Audio anchor");
  });

  it("shows unsynced plain text and never stretches lines across a mismatched duration", async () => {
    const plain = render({ status: "unsynced", provider: "lrclib", plainLyrics: "Synthetic unsynced verse\nAnother synthetic line" });
    await settle();
    plain.target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(plain.target.querySelector(".lyrics-unsynced")?.textContent).toContain("Another synthetic line");
    destroy?.(); destroy = undefined; document.body.innerHTML = "";

    const mismatch = render({ status: "synced", provider: "lrclib", match: { trackName: "Synthetic Song", artistName: "Synthetic Artist", albumName: "Synthetic Album", durationSeconds: 220, durationDeltaSeconds: 40 }, lines: [{ timeSeconds: 0, text: "Do not display mismatched lyrics" }] }, 100);
    await settle();
    mismatch.target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(mismatch.target.textContent).toContain("title, performers, or duration");
    expect(mismatch.target.textContent).not.toContain("Do not display mismatched lyrics");
    expect(mismatch.target.querySelector(".lyrics-scroll")).toBeNull();
  });

  it("reports unmatched and unavailable results without inventing lyric text", async () => {
    const result = render({ status: "unmatched", provider: "lrclib" });
    await settle();
    result.target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(result.target.textContent).toContain("title, performers, or duration");
    expect(result.target.querySelector(".lyrics-scroll")).toBeNull();
  });

  it("distinguishes missing playback metadata, invalid metadata, and provider outages", async () => {
    for (const [status, expected] of [
      ["noCurrentTrack", "No current full-track metadata"],
      ["invalidRequest", "LRCLIB needs a valid title, artist, and duration"],
      ["providerUnavailable", "LRCLIB could not be reached"],
    ] as const) {
      destroy?.(); destroy = undefined; document.body.innerHTML = "";
      const { target } = render({ status, provider: "lrclib" });
      await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
      expect(target.textContent).toContain(expected);
      expect(target.querySelector(".lyrics-scroll")).toBeNull();
    }
  });

  it("translates only after click and preserves each original timestamp and line", async () => {
    const lines = [
      { id: "source-line-1", timeSeconds: 4.25, text: "Synthetic original one" },
      { id: "source-line-2", timeSeconds: 9.5, text: "Synthetic original two" },
    ];
    const translateLyrics = vi.fn(async (sourceHash: string, requested: typeof lines) => ({
      sourceHash, targetLanguage: "es", lines: requested.map(line => ({ id: line.id, text: `Traducción ${line.id}` })),
    }));
    const { target } = render({ status: "synced", provider: "lrclib", sourceHash: "synthetic-source-hash", lines }, 10, {
      translationEnabled: true, translationCredentialPresent: true, translateLyrics,
    });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(translateLyrics).not.toHaveBeenCalled();
    target.querySelector<HTMLButtonElement>(".lyrics-translation-controls button")!.click(); await settle();
    expect(translateLyrics).toHaveBeenCalledWith("synthetic-source-hash", [
      { id: "source-line-1", timeSeconds: 4.25, text: "Synthetic original one" },
      { id: "source-line-2", timeSeconds: 9.5, text: "Synthetic original two" },
    ]);
    expect(target.querySelectorAll(".lyrics-line-row.translated-row")).toHaveLength(2);
    expect(target.textContent).toContain("Synthetic original one");
    expect(target.textContent).toContain("Traducción source-line-1");
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic original two");
    expect(lines.map(line => line.timeSeconds)).toEqual([4.25, 9.5]);
  });

  it("rejects a translation response for a different lyric source", async () => {
    const translateLyrics = vi.fn(async () => ({ sourceHash: "stale-source", targetLanguage: "es", lines: [{ id: "line-1", text: "Stale synthetic text" }] }));
    const { target } = render({ status: "synced", provider: "lrclib", sourceHash: "current-source", lines: [
      { id: "line-1", timeSeconds: 0, text: "Current synthetic text" },
    ] }, 0, { translationEnabled: true, translationCredentialPresent: true, translateLyrics });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-translation-controls button")!.click(); await settle();
    expect(target.textContent).toContain("translation did not match these lyrics");
    expect(target.textContent).not.toContain("Stale synthetic text");
    expect(target.querySelectorAll(".lyrics-line-row.translated-row")).toHaveLength(0);
  });

  it("translates unsynchronized paragraphs without adding timestamps or dropping blank lines", async () => {
    const lines = [
      { id: "paragraph-1", timeSeconds: null, text: "Synthetic plain paragraph" },
      { id: "paragraph-2", timeSeconds: null, text: "" },
      { id: "paragraph-3", timeSeconds: null, text: "Another synthetic paragraph" },
    ];
    const translateLyrics = vi.fn(async (sourceHash: string, requested: typeof lines) => ({
      sourceHash, targetLanguage: "French", lines: requested.map(line => ({ id: line.id, text: line.text ? `Phrase ${line.id}` : "" })),
    }));
    const { target } = render({ status: "unsynced", provider: "lrclib", sourceHash: "plain-source-hash", plainLyrics: "Synthetic plain paragraph\n\nAnother synthetic paragraph", lines }, 0, {
      translationEnabled: true, translationCredentialPresent: true, translateLyrics,
    });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-translation-controls button")!.click(); await settle();
    expect(translateLyrics).toHaveBeenCalledWith("plain-source-hash", lines);
    expect(target.querySelectorAll(".lyrics-plain-rows .translated-row")).toHaveLength(3);
    expect(target.textContent).toContain("Synthetic plain paragraph");
    expect(target.textContent).toContain("Phrase paragraph-3");
    expect(target.querySelectorAll(".lyrics-plain-rows .translated-row")[1]?.textContent).not.toContain("Translation unavailable");
    expect(lines.every(line => line.timeSeconds === null)).toBe(true);
  });

  it("shows actionable provider failure for the reported collaboration track", async () => {
    const reportedTrack = { id: "reported-nunca", provider: "youtube", providerId: "reported", sourceUrl: "https://youtube.test/watch?v=reported", title: "Nunca vas a comprender", artist: "Rita Payés & Elisabeth Roma", album: "Como la piel", durationSeconds: 244.631, favorite: false, provenance: {} };
    const { target, call } = render({ status: "providerUnavailable", provider: "lrclib" }, 0, {}, reportedTrack);
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(call).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("LRCLIB could not be reached");
    expect(target.textContent).toContain("Check your internet connection");
    expect(target.querySelector<HTMLButtonElement>('[role="status"] button')?.textContent).toBe("Retry");
  });
});
