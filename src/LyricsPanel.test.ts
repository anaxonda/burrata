// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import LyricsPanelScopeHarness from "./LyricsPanelScopeHarness.svelte";
import LyricsPanel, { type CurrentTrackLyrics } from "./LyricsPanel.svelte";
import {lyricsTiming} from "./lib/lyrics-timing.svelte";

let destroy: (() => void) | undefined;
let savedStorageDescriptor:PropertyDescriptor|undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; lyricsTiming.clear();if(savedStorageDescriptor){Object.defineProperty(window,"localStorage",savedStorageDescriptor);savedStorageDescriptor=undefined;}vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); await new Promise(resolve => setTimeout(resolve, 0)); await tick(); }
const track = { id: "synthetic-song", provider: "youtube", providerId: "synthetic", sourceUrl: "https://youtube.test/watch?v=synthetic", title: "Synthetic Song", artist: "Synthetic Artist", album: "Synthetic Album", durationSeconds: 180, favorite: false, provenance: {} };
function render(result: CurrentTrackLyrics, absolutePosition = 0, extra: Record<string, unknown> = {}, trackValue: typeof track & {localPath?:string} = track) {
  const target = document.createElement("div"); document.body.append(target);
  const loadLyrics = vi.fn(async () => result);
  const instance = mount(LyricsPanel, { target, props: { track: trackValue, absolutePosition, loadLyrics, ...extra } as never });
  destroy = () => unmount(instance);
  return { target, call: loadLyrics, instance };
}

describe("LyricsPanel", () => {
  it("updates the next-cue countdown immediately even when a small shift keeps the same line highlighted",async()=>{
    const {target}=render({status:"synced",provider:"lrclib",sourceHash:"countdown-source",lines:[{id:"a",timeSeconds:10,text:"Current cue"},{id:"b",timeSeconds:20,text:"Next cue"}]},15);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    expect(target.querySelector(".lyrics-timing-preview")?.textContent).toContain("5.0 s");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics later by 0.1 seconds"]')!.click();await settle();
    expect(target.querySelector("p.active")?.textContent).toBe("Current cue");
    expect(target.querySelector(".lyrics-timing-preview")?.textContent).toContain("5.1 s");
  });
  it("shares and remembers timing per recording and source, and Reset clears the saved adjustment",async()=>{
    const saved=new Map<string,string>();savedStorageDescriptor=Object.getOwnPropertyDescriptor(window,"localStorage");
    Object.defineProperty(window,"localStorage",{configurable:true,value:{getItem:(key:string)=>saved.get(key)??null,setItem:(key:string,value:string)=>saved.set(key,value),removeItem:(key:string)=>saved.delete(key)}});
    const result:CurrentTrackLyrics={status:"synced",provider:"lrclib",sourceHash:"saved-timing-source",lines:[{id:"a",timeSeconds:10,text:"Saved timing line"}]};
    const a=render(result,10.05),b=render(result,10.05);
    destroy=()=>{void unmount(a.instance);void unmount(b.instance);};
    await settle();for(const panel of [a,b])panel.target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    a.target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics later by 0.1 seconds"]')!.click();await settle();
    expect(b.target.querySelector(".lyrics-timing-stepper span")?.textContent).toBe("0.1 s later");expect(b.target.querySelector("p.active")).toBeNull();
    expect(Object.values(JSON.parse(window.localStorage.getItem("burrata.lyrics-timing.v1")!))).toEqual([expect.objectContaining({seconds:-0.1})]);
    const c=render(result,10.05);const other=render(result,10.05,{}, {...track,id:"another-recording",providerId:"another-recording"});
    destroy=()=>{for(const panel of [a,b,c,other])void unmount(panel.instance);};
    await settle();for(const panel of [c,other])panel.target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    expect(c.target.querySelector(".lyrics-timing-stepper span")?.textContent).toBe("0.1 s later");
    expect(other.target.querySelector(".lyrics-timing-stepper span")?.textContent).toBe("0.0 s");
    b.target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click();await settle();
    expect(c.target.querySelector(".lyrics-timing-stepper span")?.textContent).toBe("0.0 s");
    expect(JSON.parse(window.localStorage.getItem("burrata.lyrics-timing.v1")!)).toEqual({});
  });
  it("retries with the configured fallback only when its button is clicked",async()=>{
    const lines=[{id:"a",timeSeconds:null,text:"Transliterated source line"}];
    const translateLyrics=vi.fn(async(_hash:string,_lines:unknown,_progress:unknown,_signal:unknown,options?:{useFallback?:boolean})=>{
      if(!options?.useFallback)throw new Error("Google returned untranslated text");
      return{sourceHash:"fallback-source",targetLanguage:"en",lines:[{id:"a",text:"Meaningful translated line"}],provider:"openRouter"};
    });
    const {target}=render({status:"unsynced",provider:"lrclib",sourceHash:"fallback-source",lines},0,{translationEnabled:true,translationCredentialPresent:true,translateLyrics,translationLanguage:"en"});
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();target.querySelector<HTMLButtonElement>('[aria-label="Show translation"]')!.click();await settle();
    expect(translateLyrics.mock.calls[0][4]).toBeUndefined();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent==="Retry with fallback")!.click();await settle();
    expect(translateLyrics.mock.calls[1][4]).toEqual({useFallback:true});expect(target.textContent).toContain("Meaningful translated line");
  });
  it("aligns a chosen line against playback, source offset and audio anchor without changing source timestamps",async()=>{
    const lines=[{id:"a",timeSeconds:10,text:"First calibration line"},{id:"b",timeSeconds:14,text:"Next calibration line"}];
    const {target,call}=render({status:"synced",provider:"localLrc",sourceHash:"calibration",sourceOffsetSeconds:1.5,lines},12.5,{syncLyrics:async()=>({offsetSeconds:2})});
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-sync button")!.click();await settle();
    expect(target.querySelector("p.active")?.textContent).toBe("Next calibration line");
    target.querySelector<HTMLButtonElement>(".lyrics-timing button")!.click();await settle();
    expect(target.querySelector(".lyrics-timing-notice")?.textContent).toContain("Click a line");
    target.querySelector<HTMLButtonElement>('[data-lyric-index="0"] .lyrics-set-line')!.click();await settle();
    expect(target.querySelector("p.active")?.textContent).toBe("First calibration line");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("4.0 s later");
    expect(target.querySelector(".lyrics-set-line")).toBeNull();
    expect(lines.map(line=>line.timeSeconds)).toEqual([10,14]);expect(call).toHaveBeenCalledOnce();
    target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click();await settle();
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.0 s");
  });
  it("uses the current playback position at the line click rather than when timing mode opened",async()=>{
    const target=document.createElement("div");document.body.append(target);
    const loadLyrics=vi.fn(async()=>({status:"synced" as const,provider:"lrclib" as const,lines:[{id:"a",timeSeconds:10,text:"Click at the entrance"}]}));
    const instance=mount(LyricsPanelScopeHarness,{target,props:{initialTrack:track,initialPosition:8,loadLyrics}});destroy=()=>unmount(instance);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-timing button")!.click();await settle();
    instance.replacePosition(9);await settle();target.querySelector<HTMLButtonElement>(".lyrics-set-line")!.click();await settle();
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("1.0 s earlier");
    expect(target.querySelector("p.active")?.textContent).toBe("Click at the entrance");
  });
  it("rejects a distant line without silently clamping timing and lets Escape cancel",async()=>{
    const {target}=render({status:"synced",provider:"lrclib",lines:[{id:"far",timeSeconds:100,text:"Distant cue"}]},1);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-timing button")!.click();await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-set-line")!.click();await settle();
    expect(target.querySelector(".lyrics-timing-notice")?.textContent).toContain("within 30 seconds");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.0 s");
    target.querySelector(".lyrics-panel")!.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true}));await settle();
    expect(target.querySelector(".lyrics-set-line")).toBeNull();
  });
  it("applies manual timing immediately and retains it through advancing playback and equivalent track updates",async()=>{
    const target=document.createElement("div");document.body.append(target);
    const lines=[{id:"a",timeSeconds:10,text:"First timing line"},{id:"b",timeSeconds:20,text:"Second timing line"}];
    const loadLyrics=vi.fn(async()=>({status:"synced" as const,provider:"lrclib" as const,lines}));
    const instance=mount(LyricsPanelScopeHarness,{target,props:{initialTrack:track,initialPosition:10.05,loadLyrics}});destroy=()=>unmount(instance);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    expect(target.querySelector("p.active")?.textContent).toBe("First timing line");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics later by 0.1 seconds"]')!.click();await settle();
    expect(target.querySelector("p.active")).toBeNull();
    for(const position of [10.06,10.09,10.2,19.9,20.05]){instance.replacePosition(position);instance.replaceTrack({...track});await settle();expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.1 s later");}
    expect(target.querySelector("p.active")?.textContent).toBe("First timing line");
    instance.replacePosition(20.2);await settle();
    expect(target.querySelector("p.active")?.textContent).toBe("Second timing line");
    expect(loadLyrics).toHaveBeenCalledOnce();
  });
  it("applies source timing offset without rewriting lyric timestamps or resetting it with manual controls", async () => {
    const result: CurrentTrackLyrics = {status:"synced",provider:"localLrc",sourceOffsetSeconds:2,
      lines:[{timeSeconds:1,text:"First synthetic line"},{timeSeconds:3,text:"Next synthetic line"}]};
    const {target}=render(result,1.5);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Next synthetic line");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics later by 0.1 seconds"]')!.click();await settle();
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
    expect(target.querySelector('[aria-label="Show translation"]')).not.toBeNull();
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

  it("applies a bounded local tenth-second timing correction without changing source timestamps", async () => {
    const lines = [{ timeSeconds: 6, text: "Synthetic first line" }, { timeSeconds: 10.05, text: "Synthetic next line" }];
    const { target } = render({ status: "synced", provider: "lrclib", lines }, 10);
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic first line");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.1 seconds"]')!.click(); await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic next line");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.1 s earlier");
    expect(lines[1].timeSeconds).toBe(10.05);
    target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click(); await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic first line");
    for (let i = 0; i < 301; i++) target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.1 seconds"]')!.click();
    await settle();
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("30.0 s earlier");
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.1 seconds"]')?.disabled).toBe(true);
  });

  it("moves the highlight in both directions by 0.1 seconds and resets without rewriting source timing", async () => {
    const lines=[{timeSeconds:10,text:"Offset boundary line"},{timeSeconds:20,text:"Later source line"}];
    const {target}=render({status:"synced",provider:"localLrc",sourceOffsetSeconds:2,lines},7.95);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    expect(target.querySelector(".lyrics-scroll p.active")).toBeNull();
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.1 seconds"]')!.click();await settle();
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Offset boundary line");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.1 s earlier");
    const later=target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics later by 0.1 seconds"]')!;later.click();later.click();await settle();
    expect(target.querySelector(".lyrics-scroll p.active")).toBeNull();
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.1 s later");
    target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click();await settle();
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.0 s");
    expect(lines.map(line=>line.timeSeconds)).toEqual([10,20]);
  });

  it("adds an identified-audio anchor to manual timing and clears both with Reset", async () => {
    const syncLyrics = vi.fn(async () => ({ offsetSeconds: 2 }));
    const { target } = render({ status: "synced", provider: "lrclib", lines: [
      { timeSeconds: 6, text: "Synthetic first line" }, { timeSeconds: 12, text: "Synthetic next line" },
    ] }, 10, { syncLyrics, syncScope: "generation-1:seek-2" });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(target.querySelector(".lyrics-sync")?.textContent).toContain("a short sample from current native playback is sent");
    target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics earlier by 0.1 seconds"]')!.click();
    target.querySelector<HTMLButtonElement>(".lyrics-sync button")!.click(); await settle();
    expect(syncLyrics).toHaveBeenCalledTimes(1);
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic next line");
    expect(target.querySelector(".lyrics-timing")?.textContent).toContain("0.1 s earlier");
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
      ["providerUnavailable", "Lyrics are temporarily unavailable from LRCLIB"],
    ] as const) {
      destroy?.(); destroy = undefined; document.body.innerHTML = "";
      const { target } = render({ status, provider: "lrclib" });
      await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
      expect(target.textContent).toContain(expected);
      expect(target.querySelector(".lyrics-scroll")).toBeNull();
    }
  });

  it("pairs bilingual source lines and the icon hides and restores the second column", async () => {
    const { target } = render({ status: "unsynced", provider: "lrclib", plainLyrics: "Zora svetla\nThe morning shines\nMore tiho\nThe sea is calm\nRano leto\nThe summer is warm\nNebo sine\nThe sky is blue" });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(target.querySelector(".lyrics-unsynced")).toBeNull();
    const toggle = target.querySelector<HTMLButtonElement>('[aria-label="Show translation"]')!;
    const rows = [...target.querySelectorAll(".lyrics-line-row")];
    expect(rows).toHaveLength(4);
    expect([...rows[0]!.querySelectorAll("p")].map(p => p.textContent)).toEqual(["Zora svetla", "The morning shines"]);
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    toggle.click(); await settle();
    expect(target.querySelectorAll(".translated-line")).toHaveLength(0);
    expect(target.querySelectorAll(".translated-row")).toHaveLength(0);
    toggle.click(); await settle();
    expect(target.querySelectorAll(".translated-line")).toHaveLength(4);
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
      translationEnabled: true, translationCredentialPresent: true, translateLyrics, translationLanguage:"es",
    });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(translateLyrics).not.toHaveBeenCalled();
    target.querySelector<HTMLButtonElement>(".lyrics-translation-controls button")!.click(); await settle();
    expect(translateLyrics.mock.calls[0].slice(0,2)).toEqual(["synthetic-source-hash", [
      { id: "source-line-1", timeSeconds: 4.25, text: "Synthetic original one" },
      { id: "source-line-2", timeSeconds: 9.5, text: "Synthetic original two" },
    ]]);
    expect(target.querySelectorAll(".lyrics-line-row.translated-row")).toHaveLength(2);
    expect(target.textContent).toContain("Synthetic original one");
    expect(target.textContent).toContain("Traducción source-line-1");
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic original two");
    expect(target.querySelector(".translated-line.active")?.textContent).toBe("Traducción source-line-2");
    expect(target.querySelector('.translated-line[aria-current="true"]')?.textContent).toBe("Traducción source-line-2");
    const later=target.querySelector<HTMLButtonElement>('[aria-label="Shift lyrics later by 0.1 seconds"]')!;
    for(let i=0;i<6;i++){later.click();await settle();}
    expect(target.querySelector(".lyrics-scroll p.active")?.textContent).toBe("Synthetic original one");
    expect(target.querySelector(".translated-line.active")?.textContent).toBe("Traducción source-line-1");
    expect(target.querySelectorAll(".lyrics-scroll p.active")).toHaveLength(2);
    target.querySelector<HTMLButtonElement>(".lyrics-reset")!.click();await settle();
    expect(target.querySelector(".translated-line.active")?.textContent).toBe("Traducción source-line-2");
    expect(lines.map(line => line.timeSeconds)).toEqual([4.25, 9.5]);
    const toggle = target.querySelector<HTMLButtonElement>('[aria-label="Show translation"]')!;
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    toggle.click(); await settle();
    expect(target.querySelectorAll(".translated-line")).toHaveLength(0);
    expect(target.querySelectorAll(".lyrics-scroll p.active")).toHaveLength(1);
    expect(target.textContent).toContain("Synthetic original one");
    toggle.click(); await settle();
    expect(target.querySelectorAll(".translated-line")).toHaveLength(2);
    expect(translateLyrics).toHaveBeenCalledTimes(1);
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
    expect(target.querySelectorAll(".translated-line")?.[0]?.textContent).not.toContain("Stale synthetic text");
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
      translationEnabled: true, translationCredentialPresent: true, translateLyrics, translationLanguage:"French",
    });
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-translation-controls button")!.click(); await settle();
    expect(translateLyrics.mock.calls[0].slice(0,2)).toEqual(["plain-source-hash", lines]);
    expect(target.querySelectorAll(".lyrics-plain-rows .translated-row")).toHaveLength(3);
    expect(target.textContent).toContain("Synthetic plain paragraph");
    expect(target.textContent).toContain("Phrase paragraph-3");
    expect(target.querySelectorAll(".lyrics-plain-rows .translated-row")[1]?.textContent).not.toContain("Translation unavailable");
    expect(lines.every(line => line.timeSeconds === null)).toBe(true);
  });

  it("sorts lyric candidates by proximity to the current runtime and shows readable durations", async () => {
    const candidates = [
      {id:"a",trackName:"Synthetic Song",artistName:"First candidate",durationSeconds:174,hasSynced:false,automaticMatch:false,explanation:"Related",sourceUrl:"https://lrclib.net/api/get/1"},
      {id:"b",trackName:"Synthetic Song",artistName:"Closest candidate",durationSeconds:179,hasSynced:false,automaticMatch:false,explanation:"Related",sourceUrl:"https://lrclib.net/api/get/2"},
    ];
    const {target}=render({status:"unmatched",provider:"lrclib",candidates});
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    const select=target.querySelector<HTMLSelectElement>('[aria-label="Sort lyric results"]')!;
    expect(target.textContent).toContain("Playing 3:00");
    select.value="duration";select.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(target.querySelector(".lyric-candidates article")?.textContent).toContain("Closest candidate");
    expect(target.querySelector(".lyric-candidates article")?.textContent).toContain("2:59 · 0:01 shorter");
    expect(candidates[0].id).toBe("a");
  });

  it("keeps the same translation toggle and user choice while partial batches arrive",async()=>{
    const lines=[{id:"a",timeSeconds:1,text:"Synthetic first"},{id:"b",timeSeconds:2,text:"Synthetic second"}];
    let progress:((result:any)=>void)|undefined,finish:((result:any)=>void)|undefined;
    const translateLyrics=vi.fn((_hash:string,_lines:unknown,_progress?:any)=>{progress=_progress;return new Promise<any>(resolve=>{finish=resolve;});});
    const {target}=render({status:"synced",provider:"lrclib",sourceHash:"batch-source",lines},0,{translationEnabled:true,translationCredentialPresent:true,translationLanguage:"fr",translateLyrics});
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    const toggle=target.querySelector<HTMLButtonElement>('[aria-label="Show translation"]')!;toggle.click();await settle();
    progress!({sourceHash:"batch-source",targetLanguage:"fr",lines:[{id:"a",text:"Premier"}]});await settle();
    expect(target.textContent).toContain("Premier");expect(toggle.getAttribute("aria-pressed")).toBe("true");expect(target.querySelector('[aria-label="Show translation"]')).toBe(toggle);
    toggle.click();await settle();expect(toggle.getAttribute("aria-pressed")).toBe("false");
    finish!({sourceHash:"batch-source",targetLanguage:"fr",lines:[{id:"a",text:"Premier"},{id:"b",text:"Deuxième"}]});await settle();
    expect(toggle.getAttribute("aria-pressed")).toBe("false");expect(target.querySelector('[aria-label="Show translation"]')).toBe(toggle);
    toggle.click();await settle();expect(target.textContent).toContain("Deuxième");
  });

  it("refreshes an open radio lyric panel for the next identified song and keeps it untimed",async()=>{
    const target=document.createElement("div");document.body.append(target);
    const loadLyrics=vi.fn().mockResolvedValueOnce({status:"unsynced",provider:"lrclib",plainLyrics:"First radio text",lines:[{id:"radio-1",timeSeconds:null,text:"First radio text"}]}).mockResolvedValueOnce({status:"unsynced",provider:"lrclib",plainLyrics:"Next radio text",lines:[{id:"radio-2",timeSeconds:null,text:"Next radio text"}]});
    const radio={...track,id:"radio:station",provider:"radio",title:"First radio song"};
    const instance=mount(LyricsPanelScopeHarness,{target,props:{initialTrack:radio,loadLyrics,fullscreenRequested:false}});destroy=()=>unmount(instance);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();expect(target.textContent).toContain("First radio text");
    instance.replaceTrack({...radio,title:"Next radio song"});await settle();
    expect(loadLyrics).toHaveBeenCalledTimes(2);expect(target.textContent).toContain("Next radio text");expect(target.textContent).not.toContain("First radio text");expect(target.querySelector(".lyrics-timing")).toBeNull();
  });

  it("shows actionable provider failure for the reported collaboration track", async () => {
    const reportedTrack = { id: "reported-nunca", provider: "youtube", providerId: "reported", sourceUrl: "https://youtube.test/watch?v=reported", title: "Nunca vas a comprender", artist: "Rita Payés & Elisabeth Roma", album: "Como la piel", durationSeconds: 244.631, favorite: false, provenance: {} };
    const { target, call } = render({ status: "providerUnavailable", provider: "lrclib" }, 0, {}, reportedTrack);
    await settle(); target.querySelector<HTMLButtonElement>(".lyrics-open")!.click(); await settle();
    expect(call).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("Lyrics are temporarily unavailable from LRCLIB");
    expect(target.textContent).not.toContain("Check your internet connection");
    expect(target.querySelector<HTMLButtonElement>('[role="status"] button')?.textContent).toBe("Retry");
  });
  it("keeps extras inside the closed menu and copies or exports the displayed lyrics", async () => {
    const writeText=vi.fn(async()=>{}), exportLyrics=vi.fn(async()=>({cancelled:false})), saveLyrics=vi.fn(async()=>({saved:true}));
    const descriptor=Object.getOwnPropertyDescriptor(navigator,"clipboard");Object.defineProperty(navigator,"clipboard",{value:{writeText},configurable:true});
    try {
      const {target}=render({status:"unsynced",provider:"localLrc",sourceHash:"source-copy",plainLyrics:"A synthetic verse",lines:[{id:"copy-1",timeSeconds:null,text:"A synthetic verse"}],match:{artistName:"Synthetic credit",trackName:"Synthetic Song"}},0,{exportLyrics,saveLyrics},{...track,localPath:"/tmp/test-managed.flac"});
      await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
      const menu=target.querySelector<HTMLDetailsElement>(".lyrics-menu")!;expect(menu.open).toBe(false);expect(menu.textContent).toContain("Lyrics credit");expect(target.querySelector(".lyrics-translation-controls button")?.textContent).toBe("");expect(target.textContent).not.toContain("Display sync only");
      const button=(text:string)=>[...menu.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent===text)!;
      button("Copy lyrics").click();await settle();expect(writeText).toHaveBeenCalledWith("A synthetic verse");
      button("Save lyrics as text file").click();await settle();expect(exportLyrics).toHaveBeenCalledWith("source-copy","A synthetic verse");
      button("Embed lyrics in audio file").click();await settle();expect(saveLyrics).toHaveBeenCalledWith("source-copy");
    } finally {if(descriptor)Object.defineProperty(navigator,"clipboard",descriptor);else delete (navigator as unknown as {clipboard?:unknown}).clipboard;}
  });

  it("shows one actionable failure instead of repeating an error in every lyric row", async () => {
    const lines=[{id:"a",timeSeconds:null,text:"Original one"},{id:"b",timeSeconds:null,text:""},{id:"c",timeSeconds:null,text:"Original two"}];
    const {target}=render({status:"unsynced",provider:"lrclib",sourceHash:"failure-source",plainLyrics:"Original one\n\nOriginal two",lines},0,{translationLanguage:"fr",translationEnabled:true,translationCredentialPresent:true,translateLyrics:vi.fn(async()=>{throw new Error("Translation service returned HTTP 401");})});
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();target.querySelector<HTMLButtonElement>('[aria-label="Show translation"]')!.click();await settle();
    expect(target.querySelector(".translation-notice")?.textContent).toContain("HTTP 401");
    expect(target.textContent?.match(/HTTP 401/g)).toHaveLength(1);
    expect(target.textContent).not.toContain("Translation unavailable for this line");
    expect([...target.querySelectorAll(".translated-line")].every(line=>!line.textContent?.trim())).toBe(true);
    expect(target.textContent).toContain("Original one");expect(target.textContent).toContain("Original two");
  });
  it("invalidates loaded lyrics when artist credit or selected local file changes", async () => {
    const target=document.createElement("div");document.body.append(target);
    let text="Old artist lyric";const load=vi.fn(async()=>({status:"unsynced" as const,provider:"embeddedLyrics" as const,plainLyrics:text}));
    const instance=mount(LyricsPanelScopeHarness,{target,props:{initialTrack:track,loadLyrics:load}});destroy=()=>unmount(instance);
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();expect(target.textContent).toContain("Old artist lyric");
    text="Corrected artist lyric";instance.replaceTrack({...track,artist:"Corrected Artist"});await settle();expect(target.textContent).not.toContain("Old artist lyric");target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();expect(target.textContent).toContain(text);
    text="Preferred file lyric";instance.replaceTrack({...track,artist:"Corrected Artist",assetId:"preferred",localPath:"/tmp/preferred.flac"});await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();expect(target.textContent).toContain(text);expect(load).toHaveBeenCalledTimes(3);
  });
  it("does not reopen translation after it is hidden during a language save", async () => {
    let finish!:()=>void;const change=vi.fn(()=>new Promise<void>(resolve=>finish=resolve));
    const lines=[{id:"line",timeSeconds:0,text:"Original"}];const translate=vi.fn(async()=>({sourceHash:"toggle-language",targetLanguage:"en",lines:[{id:"line",text:"Translated"}]}));
    const {target}=render({status:"synced",provider:"lrclib",sourceHash:"toggle-language",lines},0,{translationLanguage:"en",changeTranslationLanguage:change,translationEnabled:true,translationCredentialPresent:true,translateLyrics:translate});
    await settle();target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();const toggle=target.querySelector<HTMLButtonElement>('[aria-label="Show translation"]')!;toggle.click();await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Translation language"]')!.click();await settle();const search=target.querySelector<HTMLInputElement>('[aria-label="Search translation languages"]')!;search.value="French";search.dispatchEvent(new Event("input",{bubbles:true}));await settle();target.querySelector<HTMLButtonElement>('[role="option"]')!.click();await settle();toggle.click();await settle();finish();await settle();expect(toggle.getAttribute("aria-pressed")).toBe("false");expect(translate).toHaveBeenCalledTimes(1);
  });
  it("offers several candidates and displays a selected related performance without timing", async () => {
    const candidates = [1,2].map(id => ({ id:String(id),trackName:"Synthetic Song",artistName:`Performer ${id}`,albumName:`Album ${id}`,durationSeconds:200,hasSynced:true,automaticMatch:false,explanation:"Related performance",sourceUrl:`https://lrclib.net/api/get/${id}` }));
    const find=vi.fn(async()=>({scopeId:"scope",status:"ok",candidates}));
    const choose=vi.fn(async()=>({status:"unsynced",provider:"lrclib",sourceHash:"selected",selectionNotice:"Related performance; timing disabled",match:{trackName:"Synthetic Song",artistName:"Performer 1",durationDeltaSeconds:20},lines:[{id:"line",timeSeconds:null,text:"Selected synthetic text"}]}));
    const {target}=render({status:"notFound",provider:"lrclib"},0,{findLyrics:find,chooseLyrics:choose});
    await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-open")!.click();await settle();
    target.querySelector<HTMLButtonElement>(".lyrics-other-results")!.click();await settle();
    expect(target.querySelectorAll(".lyric-candidates article")).toHaveLength(2);
    target.querySelector<HTMLButtonElement>(".lyric-candidates article button")!.click();await settle();
    expect(choose).toHaveBeenCalledWith("scope","1");
    expect(target.textContent).toContain("Selected synthetic text");
    expect(target.textContent).toContain("timing disabled");
    expect(target.textContent).not.toContain("It is not displayed");
  });

});
