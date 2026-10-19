// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
const apiCall = vi.hoisted(() => vi.fn());
vi.mock("./lib/api", () => ({ call: apiCall, desktop: () => true }));
import SearchPanel from "./SearchPanel.svelte";

let destroy: (() => void) | undefined;
const storage = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), clear: () => storage.clear() } });
afterEach(() => { vi.useRealTimers(); destroy?.(); destroy = undefined; document.body.innerHTML = ""; storage.clear(); apiCall.mockReset(); vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(SearchPanel, { target, props: { initialSources: ["library"], ...props } as never });
  destroy = () => unmount(instance);
  return target;
}

describe("SearchPanel", () => {
  it("plays only the exact inspected search occurrence and rejects stale row handles", async () => {
    const first = { id: "same-track", provider: "youtube", providerId: "video-one", sourceUrl: "https://youtube.test/watch?v=one", title: "Same Song", artist: "Artist", favorite: false, provenance: {} };
    const second = { ...first, providerId: "video-two", sourceUrl: "https://youtube.test/watch?v=two", provenance: { remoteEntryId: "duplicate-occurrence" } };
    const searchSource = vi.fn(async () => ({ sourceId: "youtube", complete: true, results: [
      { id: "row-one", kind: "track", sourceId: "youtube", title: first.title, track: first },
      { id: "row-two", kind: "track", sourceId: "youtube", title: second.title, track: second },
    ] }));
    const onPlay = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(SearchPanel, { target, props: {
      initialSources: ["youtube"], availableSources: [{ id: "youtube", name: "YouTube", enabled: true, kinds: ["track"] }],
      searchSource, onPlay,
    } as never });
    destroy = () => unmount(instance);
    const query = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    query.value = "same song"; query.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLFormElement>(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    const initial = instance.inspectAutomationSearchUi(0, 10) as { revision: string; continueSearch: boolean };
    expect(initial.continueSearch).toBe(false);
    const enabled = await instance.setAutomationContinueSearch(initial.revision, true, Number.MAX_SAFE_INTEGER) as { status: string; revision: string; enabled: boolean };
    expect(enabled).toMatchObject({ status: "complete", enabled: true });
    const snapshot = instance.inspectAutomationSearchUi(0, 10) as { revision: string; total: number; rows: { occurrenceToken: string }[] };
    expect(snapshot.total).toBe(2);
    expect(snapshot.rows[0]!.occurrenceToken).not.toBe(snapshot.rows[1]!.occurrenceToken);
    expect(instance.playAutomationSearchResult(snapshot.revision, snapshot.rows[1]!.occurrenceToken, Number.MAX_SAFE_INTEGER)).toMatchObject({ status: "dispatched" });
    expect(onPlay).toHaveBeenCalledWith(second, [first, second], 1);
    target.querySelector<HTMLButtonElement>("[aria-label='Toggle loaded track sort direction']")!.click(); await settle();
    expect(instance.playAutomationSearchResult(snapshot.revision, snapshot.rows[0]!.occurrenceToken, Number.MAX_SAFE_INTEGER)).toMatchObject({ status: "stale" });
    expect(onPlay).toHaveBeenCalledOnce();
  });
  it("rejects a prior component's revision when a later search panel remounts", async () => {
    const track = { id:"remount-track", provider:"youtube", providerId:"remount", sourceUrl:"https://youtube.test/watch?v=remount", title:"Remount Song", artist:"Artist", favorite:false, provenance:{} };
    const props = { initialSources:["youtube"], availableSources:[{id:"youtube",name:"YouTube",enabled:true,kinds:["track"]}], searchSource:async()=>({sourceId:"youtube",complete:true,results:[{id:"remount-row",kind:"track",sourceId:"youtube",title:track.title,track}]}) };
    const first = mount(SearchPanel,{target:document.body,props:props as never});
    destroy=()=>unmount(first);
    const oldRevision=(first.inspectAutomationSearchUi() as {revision:string}).revision;
    await unmount(first); document.body.innerHTML="";
    const second=mount(SearchPanel,{target:document.body,props:props as never});
    destroy=()=>unmount(second);
    const current=second.inspectAutomationSearchUi() as {revision:string;continueSearch:boolean};
    expect(current.revision).not.toBe(oldRevision);
    expect(await second.setAutomationContinueSearch(oldRevision,true,Number.MAX_SAFE_INTEGER)).toMatchObject({status:"stale"});
    expect(second.inspectAutomationSearchUi()).toMatchObject({continueSearch:false});
  });
  it("rechecks the deadline before changing preference or dispatching playback", async () => {
    const track={id:"deadline-track",provider:"youtube",providerId:"deadline",sourceUrl:"https://youtube.test/watch?v=deadline",title:"Deadline Song",artist:"Artist",favorite:false,provenance:{}};
    const onPlay=vi.fn();
    const target=document.createElement("div");document.body.append(target);
    const instance=mount(SearchPanel,{target,props:{initialSources:["youtube"],availableSources:[{id:"youtube",name:"YouTube",enabled:true,kinds:["track"]}],searchSource:async()=>({sourceId:"youtube",complete:true,results:[{id:"deadline-row",kind:"track",sourceId:"youtube",title:track.title,track}]}),onPlay} as never});destroy=()=>unmount(instance);
    const query=target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;query.value="deadline";query.dispatchEvent(new Event("input",{bubbles:true}));target.querySelector<HTMLFormElement>(".search-form")!.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true}));await settle();
    const snapshot=instance.inspectAutomationSearchUi() as {revision:string;rows:{occurrenceToken:string}[]};
    const now=vi.spyOn(Date,"now");
    now.mockReturnValueOnce(100).mockReturnValueOnce(100).mockReturnValueOnce(2_000);
    expect(await instance.setAutomationContinueSearch(snapshot.revision,true,1_000)).toMatchObject({status:"expired"});
    expect(instance.inspectAutomationSearchUi()).toMatchObject({continueSearch:false});
    expect(storage.get("burrata.discovery.continue-search.v1")).not.toBe("true");
    now.mockReturnValueOnce(100).mockReturnValueOnce(100).mockReturnValueOnce(2_000);
    expect(instance.playAutomationSearchResult(snapshot.revision,snapshot.rows[0]!.occurrenceToken,1_000)).toMatchObject({status:"expired"});
    expect(onPlay).not.toHaveBeenCalled();
    now.mockRestore();
  });
  it("reports the playable excerpt duration for a ranged search result", async () => {
    const parent={id:"parent-mix",provider:"youtube",providerId:"mix",sourceUrl:"https://youtube.test/watch?v=mix",title:"Long Mix",artist:"Artist",durationSeconds:600,favorite:false,provenance:{}};
    const excerpt={...parent,id:"chapter:parent:part",provider:"chapter",title:"Part",durationSeconds:600,playbackRange:{parentTrackId:parent.id,chapterId:"part",sourceUrl:parent.sourceUrl,startTime:45,endTime:80,parentTitle:parent.title}};
    const target=document.createElement("div");document.body.append(target);
    const instance=mount(SearchPanel,{target,props:{initialSources:["youtube"],availableSources:[{id:"youtube",name:"YouTube",enabled:true,kinds:["track"]}],searchSource:async()=>({sourceId:"youtube",complete:true,results:[{id:"chapter-row",kind:"track",sourceId:"youtube",title:excerpt.title,track:excerpt}]})} as never});destroy=()=>unmount(instance);
    const query=target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;query.value="part";query.dispatchEvent(new Event("input",{bubbles:true}));target.querySelector<HTMLFormElement>(".search-form")!.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true}));await settle();
    const snapshot=instance.inspectAutomationSearchUi() as {rows:{durationSeconds:number|null}[]};
    expect(snapshot.rows[0]!.durationSeconds).toBe(35);
  });

  it("uses unique revisions across remounts and byte-bounds Unicode metadata safely", async () => {
    const track = { id: "unicode-track", provider: "youtube", providerId: "unicode", sourceUrl: "https://youtube.test/watch?v=unicode", title: "🎵".repeat(100), artist: "Ж界".repeat(150), favorite: false, provenance: {} };
    const props = { initialSources: ["youtube"], availableSources: [{ id: "youtube", name: "YouTube", enabled: true, kinds: ["track"] }], searchSource: async () => ({ sourceId: "youtube", complete: true, results: [{ id: "unicode-row", kind: "track", sourceId: "youtube", title: track.title, track }] }) };
    const first = mount(SearchPanel, { target: document.body, props: props as never });
    destroy = () => unmount(first);
    const query = document.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    query.value = "界".repeat(200); query.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector<HTMLFormElement>(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    const before = first.inspectAutomationSearchUi() as { revision: string; query: string; rows: { title: string; artist: string }[] };
    expect(new TextEncoder().encode(before.query).byteLength).toBeLessThanOrEqual(500);
    expect(before.query.endsWith("\uFFFD")).toBe(false);
    expect(new TextEncoder().encode(before.rows[0]!.title).byteLength).toBeLessThanOrEqual(300);
    expect(new TextEncoder().encode(before.rows[0]!.artist).byteLength).toBeLessThanOrEqual(300);
    await unmount(first); document.body.innerHTML = "";
    const second = mount(SearchPanel, { target: document.body, props: props as never });
    destroy = () => unmount(second);
    const after = second.inspectAutomationSearchUi() as { revision: string; continueSearch: boolean };
    expect(after.revision).not.toBe(before.revision);
    expect(await second.setAutomationContinueSearch(before.revision, true, Number.MAX_SAFE_INTEGER)).toMatchObject({ status: "stale" });
    expect(second.inspectAutomationSearchUi()).toMatchObject({ continueSearch: false });
  });
  it("keeps primary search controls visible and collapses optional loaded-track filters", async () => {
    const track = { id: "compact-track", provider: "spotify", providerId: "compact-track", sourceUrl: "https://open.spotify.com/track/compact-track", artist: "Artist", title: "Compact result", favorite: false, provenance: {} };
    const target = render({ initialSources: ["spotify"], availableSources: [
      { id: "library", name: "My library", enabled: true, kinds: ["track"] },
      { id: "spotify", name: "Spotify", enabled: true, kinds: ["track"] },
    ], searchSource: async () => ({ complete: false, nextCursor: "more", results: [{ id: track.id, kind: "track", sourceId: "spotify", title: track.title, track }] }) });
    await settle();
    const source = target.querySelector<HTMLSelectElement>("[aria-label='Search in']")!;
    const kindTabs = target.querySelector(".filters")!;
    expect(Boolean(source.compareDocumentPosition(kindTabs) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true);
    const query = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    query.value = "compact"; query.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLFormElement>(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const details = target.querySelector<HTMLDetailsElement>("details.loaded-track-filters")!;
    expect(details.open).toBe(false);
    expect(target.querySelector(".track-toolbar > fieldset.track-controls [aria-label='Sort loaded tracks']")).not.toBeNull();
    expect(target.querySelector(".track-toolbar > .collect-more")).not.toBeNull();
    const continuation = details.querySelector<HTMLInputElement>(".continue-setting input")!;
    details.querySelector("summary")!.click(); continuation.click();
    details.querySelector("summary")!.click(); details.querySelector("summary")!.click();
    expect(details.open).toBe(true);
    expect(continuation.checked).toBe(true);
    const minimum = details.querySelector<HTMLInputElement>("[aria-label='Minimum loaded track duration']")!;
    minimum.value = "0:01"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    details.querySelector("summary")!.click();
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")!.getAttribute("aria-label")).toContain("active duration filters");
  });
  it("clears restored account-owned rows and cursors after an account change without issuing a replacement query", async () => {
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    let account="first-account";
    apiCall.mockImplementation(async(command:string)=>command==="get_managed_provider_session_status"?{spotifyAccountId:account,accountRevisions:{spotify:account==="first-account"?1:2}}:{});
    const searchSource=vi.fn(async()=>({sourceId:"spotify",complete:false,nextCursor:"private-page-two",results:[{id:"owned-row",kind:"track",sourceId:"spotify",title:"Old private result",track:{id:"old",provider:"spotify",providerId:"aaaaaaaaaaaaaaaaaaaaaa",sourceUrl:"https://open.spotify.com/track/aaaaaaaaaaaaaaaaaaaaaa",provenance:{},title:"Old private result",artist:"Artist",favorite:false,durationSeconds:240}}]}));
    const props={query:"Previous query",initialSources:["spotify"],availableSources:[{id:"spotify",name:"Spotify",enabled:true,kinds:["track"]}],searchSource};
    const first=mount(SearchPanel,{target:document.body,props});await settle();await settle();
    expect(document.body.textContent).toContain("Old private result");
    first.captureNavigationSnapshot("account-route");const reads=searchSource.mock.calls.length;
    await unmount(first);document.body.innerHTML="";account="second-account";
    const second=mount(SearchPanel,{target:document.body,props:{...props,navigationSnapshotKey:"account-route"}});destroy=()=>unmount(second);await settle();
    await second.restoreNavigationSnapshot("account-route");await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Previous query");
    expect(document.body.textContent).not.toContain("Old private result");expect(document.querySelector(".load-more")).toBeNull();
    expect(document.body.textContent).toContain("account changed");expect(searchSource).toHaveBeenCalledTimes(reads);
    const retained=providerNavigationSnapshots.get<{rowsBySource:Record<string,unknown[]>;cursors:Record<string,string>}>("search-panel","account-route","search-routes");
    expect(retained?.rowsBySource.spotify).toBeUndefined();expect(retained?.cursors.spotify).toBeUndefined();
    providerNavigationSnapshots.clear();
  });

  it("drops oversized inspected chapter metadata while preserving search rows and pagination", async () => {
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    const row={id:"chapter-result",kind:"track",sourceId:"soundcloud",title:"Chapter source",url:"https://example.test/source",track:{id:"chapter-source",provider:"youtube",providerId:"chapter-source",sourceUrl:"https://example.test/source",title:"Chapter source",artist:"Artist",favorite:false,provenance:{}}};
    const searchSource=vi.fn(async()=>({sourceId:"soundcloud",complete:false,nextCursor:"next-page",results:[row]}));
    const enrichSource=vi.fn(async()=>({id:"candidate",provider:"youtube",url:row.url,title:row.title,formats:[],permission:"unknown",confidence:"high",score:1,reasons:[],official:false,chapters:Array.from({length:10001},(_,i)=>({id:`chapter-${i}`,title:`Chapter ${i}`,startTime:i*2,endTime:i*2+1,confidence:"high"}))}));
    const instance=mount(SearchPanel,{target:document.body,props:{query:"chapters",initialSources:["soundcloud"],availableSources:[{id:"soundcloud",name:"SoundCloud",enabled:true,kinds:["track"]}],searchSource,enrichSource} as never});destroy=()=>unmount(instance);
    await settle();document.querySelector<HTMLButtonElement>(".chapter-badge")!.click();await settle();
    instance.captureNavigationSnapshot("chapter-budget");
    const saved=providerNavigationSnapshots.get<{rowsBySource:Record<string,unknown[]>;cursors:Record<string,string>;chaptersByRow:Record<string,unknown>;chapterMetadataEvicted:boolean}>("search-panel","chapter-budget","search-routes")!;
    expect(enrichSource).toHaveBeenCalledOnce();expect(saved.rowsBySource.soundcloud).toHaveLength(1);expect(saved.cursors.soundcloud).toBe("next-page");
    expect(saved.chaptersByRow).toEqual({});expect(saved.chapterMetadataEvicted).toBe(true);expect(providerNavigationSnapshots.rows).toBe(1);
    providerNavigationSnapshots.clear();
  });

  it("adds a Spotify playlist to the local library from its search result", async () => {
    const row = { id: "playlist-1", kind: "playlist", sourceId: "spotify", title: "Saved mix", url: "https://open.spotify.com/playlist/playlist-1" };
    const searchSource = vi.fn(async () => ({ sourceId: "spotify", complete: true, results: [row] }));
    const onAddRemotePlaylist = vi.fn();
    const target = render({
      initialSources: ["spotify"], availableSources: [{ id: "spotify", name: "Spotify", enabled: true, kinds: ["track", "artist", "album", "playlist"] }],
      searchSource, onAddRemotePlaylist, onRemoteResult: vi.fn(),
    });
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "saved mix"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    target.querySelector<HTMLButtonElement>(".add-playlist-result")!.click(); await settle();
    expect(onAddRemotePlaylist).toHaveBeenCalledWith(row, expect.objectContaining({ signal: expect.any(AbortSignal), onProgress: expect.any(Function) }));
    expect(target.textContent).toContain("Added to library");
  });

  it("adds selected track search occurrences without collapsing duplicate recordings", async () => {
    const first = { id: "same-recording", provider: "youtube", providerId: "youtube:one", sourceUrl: "https://youtube.test/watch?v=one", artist: "Artist", title: "Song", favorite: false, provenance: { remoteEntryId: "one" } };
    const second = { ...first, providerId: "youtube:two", sourceUrl: "https://youtube.test/watch?v=two", provenance: { remoteEntryId: "two" } };
    const searchSource = vi.fn(async () => ({ sourceId: "youtube", complete: true, results: [
      { id: "occurrence-one", kind: "track", sourceId: "youtube", title: "Song", track: first },
      { id: "occurrence-two", kind: "track", sourceId: "youtube", title: "Song", track: second },
    ] }));
    const onPlaylistTracks = vi.fn();
    const target = render({
      initialSources: ["youtube"], availableSources: [{ id: "youtube", name: "YouTube", enabled: true, kinds: ["track", "video", "playlist", "channel"] }],
      searchSource, onPlaylistTracks,
    });
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "song"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    expect(target.querySelectorAll<HTMLInputElement>('.search-track-select input')).toHaveLength(0);
    target.querySelector<HTMLButtonElement>('.start-search-selection')!.click(); await settle();
    const checks = [...target.querySelectorAll<HTMLInputElement>('.search-track-select input')];
    expect(checks).toHaveLength(2);
    checks[1]!.click(); checks[0]!.click();
    await settle();
    target.querySelector<HTMLButtonElement>('.add-search-selected')!.click();
    expect(onPlaylistTracks).toHaveBeenCalledWith([first, second]);
  });
  it("keeps search-track checkboxes hidden until Select and clears the mode on Cancel or query replacement", async () => {
    const track = { id: "track-one", provider: "youtube", providerId: "one", artist: "Artist", title: "Song", favorite: false };
    const searchSource = vi.fn(async () => ({ sourceId: "youtube", complete: true, results: [{ id: "row-one", kind: "track", sourceId: "youtube", title: "Song", track }] }));
    const target = render({
      initialSources: ["youtube"], availableSources: [{ id: "youtube", name: "YouTube", enabled: true, kinds: ["track"] }],
      searchSource, onPlaylistTracks: vi.fn(),
    });
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "song"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    expect(target.querySelectorAll<HTMLInputElement>('.search-track-select input')).toHaveLength(0);
    target.querySelector<HTMLButtonElement>('.start-search-selection')!.click(); await settle();
    const checkbox = target.querySelector<HTMLInputElement>('.search-track-select input')!;
    checkbox.click();
    expect(checkbox.checked).toBe(true);
    target.querySelector<HTMLButtonElement>('.cancel-search-selection')!.click(); await settle();
    expect(target.querySelectorAll<HTMLInputElement>('.search-track-select input')).toHaveLength(0);
    expect(target.querySelector('.start-search-selection')).not.toBeNull();
    target.querySelector<HTMLButtonElement>('.start-search-selection')!.click(); await settle();
    target.querySelector<HTMLInputElement>('.search-track-select input')!.click();
    input.value = "different query"; input.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll<HTMLInputElement>('.search-track-select input')).toHaveLength(0);
    expect(target.querySelector('.start-search-selection')).not.toBeNull();
  });
  it("runs an automation search through the selected source and reports bounded counts", async () => {
    const onAutomationSearchComplete = vi.fn();
    apiCall.mockImplementation(async (command: string) => {
      if (command === "search_source") return { complete: true, results: [
        { id: "row-1", kind: "track", sourceId: "musicbrainz", title: "Result" },
        { id: "row-2", kind: "artist", sourceId: "musicbrainz", title: "Artist" },
      ] };
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({
      query: "Daft Punk",
      initialSources: ["musicbrainz"],
      availableSources: [{ id: "musicbrainz", name: "MusicBrainz", enabled: true, kinds: ["track", "artist", "album"] }],
      automationSearch: { requestId: "auto-1", query: "Daft Punk", sourceId: "musicbrainz", kind: "tracks" },
      onAutomationSearchComplete,
    });
    await settle();
    await new Promise(resolve => setTimeout(resolve, 0));
    await settle();
    expect(apiCall).toHaveBeenCalledTimes(1);
    expect(apiCall.mock.calls[0][0]).toBe("search_source");
    expect(apiCall.mock.calls[0][1]).toEqual(expect.objectContaining({ query: "Daft Punk", sourceIds: ["musicbrainz"], kinds: ["track"] }));
    expect(onAutomationSearchComplete).toHaveBeenCalledTimes(1);
    expect(onAutomationSearchComplete).toHaveBeenCalledWith({
      requestId: "auto-1", status: "complete", sourceId: "musicbrainz", kind: "tracks", resultCounts: { musicbrainz: 2 },
      sourceCoverage: { musicbrainz: { hasMore: false, incomplete: false } },
    });
    expect(JSON.stringify(onAutomationSearchComplete.mock.calls[0][0])).not.toContain("Daft Punk");
  });

  it("reports partial automation coverage without changing operation status", async () => {
    const onAutomationSearchComplete = vi.fn();
    apiCall.mockImplementation(async (command: string) => {
      if (command === "search_source") return { complete: false, results: [
        { id: "partial", kind: "artist", sourceId: "musicbrainz", title: "Partial artist" },
      ] };
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({
      initialSources: ["musicbrainz"],
      availableSources: [{ id: "musicbrainz", name: "MusicBrainz", enabled: true, kinds: ["track", "artist", "album"] }],
      automationSearch: { requestId: "auto-partial", query: "Artist", sourceId: "musicbrainz", kind: "artists" },
      onAutomationSearchComplete,
    });
    await settle(); await new Promise(resolve => setTimeout(resolve, 0)); await settle();
    expect(onAutomationSearchComplete).toHaveBeenCalledWith({
      requestId: "auto-partial", status: "complete", sourceId: "musicbrainz", kind: "artists", resultCounts: { musicbrainz: 1 },
      sourceCoverage: { musicbrainz: { hasMore: false, incomplete: true } },
    });
    expect(target.textContent).toContain("Incomplete results; no continuation is available");
  });

  it("reports per-source continuation coverage for automation searches", async () => {
    const onAutomationSearchComplete = vi.fn();
    apiCall.mockImplementation(async (command: string) => {
      if (command === "search_source") return { complete: false, nextCursor: "next-page", results: [] };
      throw new Error(`Unexpected command: ${command}`);
    });
    render({
      initialSources: ["musicbrainz"],
      availableSources: [{ id: "musicbrainz", name: "MusicBrainz", enabled: true, kinds: ["track"] }],
      automationSearch: { requestId: "auto-more", query: "Track", sourceId: "musicbrainz", kind: "tracks" },
      onAutomationSearchComplete,
    });
    await settle(); await new Promise(resolve => setTimeout(resolve, 0)); await settle();
    expect(onAutomationSearchComplete).toHaveBeenCalledWith(expect.objectContaining({
      requestId: "auto-more", status: "complete",
      sourceCoverage: { musicbrainz: { hasMore: true, incomplete: true } },
    }));
  });

  it("completes an automation search as cancelled exactly once", async () => {
    const pending = deferred<unknown>();
    const onAutomationSearchComplete = vi.fn();
    apiCall.mockImplementation(async (command: string) => {
      if (command === "search_source") return pending.promise;
      if (command === "cancel_remote_read") { pending.reject(new Error("cancelled")); return true; }
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({
      query: "Artist",
      initialSources: ["musicbrainz"],
      availableSources: [{ id: "musicbrainz", name: "MusicBrainz", enabled: true, kinds: ["track", "artist", "album"] }],
      automationSearch: { requestId: "auto-cancel", query: "Artist", sourceId: "musicbrainz", kind: "tracks" },
      onAutomationSearchComplete,
    });
    await settle();
    target.querySelector<HTMLButtonElement>(".search-form button[type=button]")!.click();
    await settle();
    expect(onAutomationSearchComplete).toHaveBeenCalledTimes(1);
    expect(onAutomationSearchComplete).toHaveBeenCalledWith({
      requestId: "auto-cancel", status: "cancelled", sourceId: "musicbrainz", kind: "tracks", resultCounts: {}, errorCode: "cancelled",
    });
  });

  it("cancels a stalled collection read and preserves already loaded results", async () => {
    const page = deferred<unknown>();
    const track = { id: "loaded", provider: "library", providerId: "loaded", sourceUrl: "local:loaded", artist: "Artist", title: "Loaded track", favorite: false, provenance: {} };
    apiCall.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "search_source") return args?.cursor ? page.promise : { complete: false, nextCursor: "page-2", results: [{ id: "loaded", kind: "track", sourceId: "library", title: "Loaded track", track }] };
      if (command === "cancel_remote_read") { page.reject(new Error("Remote read was canceled")); return true; }
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({ initialSources: ["library"] }); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "loaded"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    target.querySelector<HTMLInputElement>(".continue-setting input")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".collect-more")!.click(); await settle();
    const requestId = apiCall.mock.calls.find(([command, args]) => command === "search_source" && (args as Record<string, unknown>)?.cursor === "page-2")?.[1]?.requestId;
    target.querySelector<HTMLButtonElement>(".collection-status button")!.click(); await settle();
    expect(apiCall).toHaveBeenCalledWith("cancel_remote_read", { requestId });
    expect(target.textContent).toContain("Collection cancelled; loaded results kept.");
    expect(target.textContent).toContain("Loaded track");
    expect(target.textContent).not.toContain("Unavailable");
  });

  it("cancels a hung collection at its deadline and retains its loaded rows", async () => {
    vi.useFakeTimers();
    const page = deferred<unknown>();
    const track = { id: "loaded", provider: "library", providerId: "loaded", sourceUrl: "local:loaded", artist: "Artist", title: "Loaded track", favorite: false, provenance: {} };
    apiCall.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "search_source") return args?.cursor ? page.promise : { complete: false, nextCursor: "page-2", results: [{ id: "loaded", kind: "track", sourceId: "library", title: "Loaded track", track }] };
      if (command === "cancel_remote_read") { page.reject(new Error("Remote read was canceled")); return true; }
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({ initialSources: ["library"] }); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "loaded"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    target.querySelector<HTMLInputElement>(".continue-setting input")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".collect-more")!.click(); await settle();
    const requestId = apiCall.mock.calls.find(([command, args]) => command === "search_source" && (args as Record<string, unknown>)?.cursor === "page-2")?.[1]?.requestId;
    await vi.advanceTimersByTimeAsync(120_000); await settle();
    expect(apiCall).toHaveBeenCalledWith("cancel_remote_read", { requestId });
    expect(target.textContent).toContain("Collection stopped at the two-minute limit; loaded results kept.");
    expect(target.textContent).toContain("Loaded track");
  });

  it("allows a new collection action after an overlong all-kind page reaches its result budget", async () => {
    const track = { id: "loaded", provider: "library", providerId: "loaded", sourceUrl: "local:loaded", artist: "Artist", title: "Loaded track", favorite: false, provenance: {} };
    const duplicateArtist = { id: "same-artist", kind: "artist", sourceId: "library", title: "Artist result" };
    const searchSource = vi.fn(async ({ cursor }: { cursor?: string | null }) => {
      if (!cursor) return { complete: false, nextCursor: "page-1", results: [{ id: "loaded", kind: "track", sourceId: "library", title: "Loaded track", track }] };
      if (cursor === "page-1") return { complete: false, nextCursor: "page-2", results: Array.from({ length: 10_001 }, () => duplicateArtist) };
      return { complete: true, results: [{ ...duplicateArtist, id: "later-artist", title: "Later result" }] };
    });
    const target = render({
      initialSources: ["library"],
      availableSources: [{ id: "library", name: "Library", enabled: true, kinds: ["track", "artist"] }],
      searchSource,
    });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "loaded"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    target.querySelector<HTMLButtonElement>(".collect-more")!.click(); await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ cursor: "page-1", limit: 30 }));
    expect(target.textContent).toContain("Artist result");

    target.querySelector<HTMLButtonElement>(".collect-more")!.click(); await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ cursor: "page-2" }));
    expect(target.textContent).toContain("Later result");
  });

  it("searches the selected source progressively and renders compact track actions", async () => {
    const track = { id: "t1", provider: "library", providerId: "t1", sourceUrl: "local:t1", artist: "A Artist", title: "A Song", favorite: false, provenance: {} };
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [{ id: "t1", kind: "track", sourceId: "library", title: "A Song", track }] }));
    const onPlay = vi.fn();
    const target = render({ searchSource, onPlay });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "A Song"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "A Song", sourceIds: ["library"], limit: 30 }));
    expect(target.textContent).toContain("A Song");
    expect(target.querySelectorAll(".compact-track .shared-track-list li")).toHaveLength(1);
    expect(target.querySelector(".compact-track .track-list-viewport")?.classList.contains("playlist")).toBe(true);
    expect(target.querySelector<HTMLButtonElement>('.compact-track .position-play[aria-label="Play A Song"]')).not.toBeNull();
    expect(target.querySelector<HTMLButtonElement>(".compact-track .more-trigger")?.getAttribute("aria-label")).toBe("Actions for A Song");
    const albumSort = [...target.querySelectorAll<HTMLOptionElement>("[aria-label='Sort loaded tracks'] option")].find(option => option.value === "album");
    expect(albumSort?.disabled).toBe(true);
    expect(albumSort?.textContent).toBe("Album (unavailable)");
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Download A Song"]')).not.toBeNull();
    target.querySelector<HTMLButtonElement>('.compact-track .row-actions .icon-action[aria-label="Download A Song"]')!.click();
    target.querySelector<HTMLButtonElement>('.compact-track .position-play[aria-label="Play A Song"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(track);
  });

  it("uses playlist-style rows for video and track results with left playback and ordered continuation", async () => {
    const video = { id: "video-result", provider: "youtube", providerId: "video-result", sourceUrl: "https://youtube.test/watch?v=video", title: "BaBa Extended Mix", artist: "DJ Example", artworkUrl: "https://img.example/video.jpg", durationSeconds: 240, favorite: false, provenance: { sourceKind: "audioSearch" } };
    const catalogTrack = { id: "catalog-result", provider: "youtube", providerId: "catalog-result", sourceUrl: "https://youtube.test/watch?v=catalog", title: "Second Track", artist: "Another Artist", favorite: true, provenance: {} };
    const onPlay = vi.fn(), onFavorite = vi.fn(), onDownload = vi.fn();
    const searchSource = vi.fn(async () => ({ sourceId: "youtube", complete: true, results: [
      { id: "video-row", kind: "video", sourceId: "youtube", title: video.title, url: video.sourceUrl, track: video },
      { id: "track-row", kind: "track", sourceId: "youtube", title: catalogTrack.title, track: catalogTrack },
    ] }));
    const target = render({
      initialSources: ["youtube"],
      availableSources: [{ id: "youtube", name: "YouTube", enabled: true, kinds: ["track", "video"] }],
      searchSource, onPlay, onFavorite, onDownload, playingTrack: catalogTrack,
    });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "extended mix"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();

    const lists = [...target.querySelectorAll<HTMLElement>(".compact-track .track-list-viewport")];
    expect(lists).toHaveLength(2);
    expect(lists.every(list => list.classList.contains("playlist"))).toBe(true);
    expect([...target.querySelectorAll(".compact-track .track-position")].map(node => node.textContent)).toEqual(["1", "2"]);
    expect(target.querySelectorAll(".compact-track .position-play")).toHaveLength(2);
    expect(target.querySelector(".compact-track .shared-track-list li.current")?.textContent).toContain("Second Track");
    expect(target.querySelector('[aria-label="Download BaBa Extended Mix"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Add BaBa Extended Mix to favorites"]')).not.toBeNull();

    target.querySelector<HTMLInputElement>(".continue-setting input")!.click();
    await settle();
    target.querySelector<HTMLButtonElement>('.position-play[aria-label="Play BaBa Extended Mix"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(video, [video, catalogTrack], 0);
    target.querySelector<HTMLButtonElement>('[aria-label="Add BaBa Extended Mix to favorites"]')!.click();
    target.querySelector<HTMLButtonElement>('[aria-label="Download BaBa Extended Mix"]')!.click();
    expect(onFavorite).toHaveBeenCalledWith(video);
    expect(onDownload).toHaveBeenCalledWith(video);
  });

  it("keeps sort fields from the loaded set available when duration filters hide their rows", async () => {
    const withTempo = { id: "short-tempo", provider: "library", providerId: "short-tempo", sourceUrl: "local:short-tempo", artist: "Artist", title: "Short with tempo", favorite: false, provenance: {}, durationSeconds: 60, bpm: 124 };
    const withoutTempo = { id: "long-no-tempo", provider: "library", providerId: "long-no-tempo", sourceUrl: "local:long-no-tempo", artist: "Artist", title: "Long without tempo", favorite: false, provenance: {}, durationSeconds: 120 };
    const target = render({
      initialSources: ["library"],
      availableSources: [{ id: "library", name: "Library", enabled: true, kinds: ["track"] }],
      searchSource: async () => ({ sourceId: "library", complete: true, results: [
        { id: "short-tempo", kind: "track", sourceId: "library", title: withTempo.title, track: withTempo },
        { id: "long-no-tempo", kind: "track", sourceId: "library", title: withoutTempo.title, track: withoutTempo },
      ] }),
    });
    await settle();
    const query = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    query.value = "tempo"; query.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(target.textContent).toContain("Short with tempo");
    expect(target.textContent).toContain("Long without tempo");
    const bpmOption = () => target.querySelector<HTMLSelectElement>("[aria-label='Sort loaded tracks']")?.querySelector<HTMLOptionElement>('option[value="bpm"]');
    expect(bpmOption()?.disabled).toBe(false);

    const minimum = target.querySelector<HTMLInputElement>("[aria-label='Minimum loaded track duration']")!;
    minimum.value = "90"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.textContent).toContain("Long without tempo");
    expect(target.textContent).not.toContain("Short with tempo");
    expect(bpmOption()?.disabled).toBe(false);
    expect(bpmOption()?.textContent).toBe("BPM");
  });

  it("keeps a large mixed result set bounded and scrolls to the correct row", async () => {
    const results = Array.from({ length: 1_000 }, (_, index) => {
      const title = `Virtual result ${index + 1}`;
      if (index % 2) return { id: `row-${index}`, kind: "video", sourceId: "library", title, subtitle: "Creator" };
      const track = { id: `track-${index}`, provider: "library", providerId: `track-${index}`, sourceUrl: `local:${index}`, artist: "Artist", title, favorite: false, provenance: {} };
      return { id: `row-${index}`, kind: "track", sourceId: "library", title, track };
    });
    const target = render({ searchSource: async () => ({ complete: true, results }) });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "virtual"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const panel = target.querySelector<HTMLElement>(".search-panel")!;
    const virtualResults = target.querySelector<HTMLElement>(".virtual-results")!;
    Object.defineProperty(panel, "clientHeight", { configurable: true, value: 600 });
    let simulatedScrollTop = panel.scrollTop;
    Object.defineProperty(panel, "scrollTop", { configurable: true, get: () => simulatedScrollTop, set: (value: number) => { simulatedScrollTop = value; } });
    panel.getBoundingClientRect = () => ({ top: 0 } as DOMRect);
    virtualResults.getBoundingClientRect = () => ({ top: 200 - panel.scrollTop } as DOMRect);
    expect(target.querySelectorAll(".virtual-result-item").length).toBeLessThan(40);
    expect(target.textContent).toContain("Virtual result 1");
    expect(target.textContent).not.toContain("Virtual result 999");

    // SearchPanel keeps nontrack hits before sorted track rows in each source group.
    panel.scrollTop = 200 + 500 * 76 + 350 * 56;
    panel.dispatchEvent(new Event("scroll"));
    await new Promise(resolve => setTimeout(resolve, 30));
    await settle();
    expect(target.querySelectorAll(".virtual-result-item").length).toBeLessThan(40);
    expect(target.textContent).toContain("Virtual result 701");
    expect(target.textContent).not.toContain("Virtual result 1");
    const renderedItems = [...target.querySelectorAll<HTMLElement>(".virtual-result-item")];
    const lastRendered = renderedItems[renderedItems.length - 1]!;
    const nextIndex = Number(lastRendered.dataset.virtualIndex) + 1;
    const controls = [...lastRendered.querySelectorAll<HTMLElement>("button:not(:disabled),a[href],input:not(:disabled),select:not(:disabled),textarea:not(:disabled),summary")].filter(control => !control.closest("details:not([open])") || control.matches("summary"));
    const lastControl = controls[controls.length - 1]!;
    // Track actions now use the shared menu trigger rather than a details summary.
    expect(lastControl.matches(".more-trigger")).toBe(true);
    lastControl.focus();
    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    lastControl.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    await settle();
    expect(target.querySelector(`.virtual-result-item[data-virtual-index="${nextIndex}"]`)?.contains(document.activeElement)).toBe(true);
  });

  it("recalculates the bounded window when the scroll viewport resizes", async () => {
    const observers: { targets: Set<Element>; trigger: (target: Element) => void }[] = [];
    const OriginalResizeObserver = globalThis.ResizeObserver;
    class ResizeObserverShim {
      targets = new Set<Element>();
      constructor(private callback: ResizeObserverCallback) {
        observers.push({ targets: this.targets, trigger: target => this.callback([{ target, contentRect: { width: 800, height: 100 } } as ResizeObserverEntry], this as unknown as ResizeObserver) });
      }
      observe(target: Element) { this.targets.add(target); }
      unobserve(target: Element) { this.targets.delete(target); }
      disconnect() { this.targets.clear(); }
    }
    Object.defineProperty(globalThis, "ResizeObserver", { configurable: true, value: ResizeObserverShim });
    try {
      const results = Array.from({ length: 100 }, (_, index) => ({ id: `video-${index}`, kind: "video", sourceId: "library", title: `Video ${index}` }));
      const target = render({ searchSource: async () => ({ complete: true, results }) });
      await settle();
      const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
      input.value = "resize"; input.dispatchEvent(new Event("input", { bubbles: true }));
      target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await settle();
      const panel = target.querySelector<HTMLElement>(".search-panel")!;
      const before = target.querySelectorAll(".virtual-result-item").length;
      Object.defineProperty(panel, "clientHeight", { configurable: true, value: 100 });
      const observer = observers.find(entry => entry.targets.has(panel));
      expect(observer).toBeDefined();
      observer!.trigger(panel);
      await new Promise(resolve => setTimeout(resolve, 30));
      await settle();
      expect(target.querySelectorAll(".virtual-result-item").length).toBeLessThan(before);
    } finally {
      Object.defineProperty(globalThis, "ResizeObserver", { configurable: true, value: OriginalResizeObserver });
    }
  });

  it("disables result kinds Spotify does not declare and explains why", async () => {
    const searchSource = vi.fn(async () => ({ complete: true, results: [] }));
    const target = render({
      initialSources: ["spotify"],
      availableSources: [{ id: "spotify", name: "Spotify", enabled: true, kinds: ["profile"] }],
      searchSource,
    });
    await settle();
    const allButton = target.querySelector<HTMLButtonElement>('.filters button');
    expect(allButton?.textContent).toBe("All");
    expect(allButton?.disabled).toBe(false);
    const profileButton = [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Profiles");
    expect(profileButton?.disabled).toBe(false);
    for (const label of ["Tracks", "Artists", "Albums", "Videos / Sets", "Playlists", "Channels"]) {
      const button = [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(item => item.textContent === label);
      expect(button?.disabled).toBe(true);
      expect(button?.getAttribute("aria-describedby")).toBe("unsupported-result-kinds");
    }
    expect(target.querySelector("#unsupported-result-kinds")?.textContent).toContain("Spotify does not support tracks, artists");
    const tracks = [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Tracks")!;
    tracks.click();
    expect(target.querySelector<HTMLButtonElement>('.filters button[aria-pressed="true"]')?.textContent).toBe("All");
    expect(searchSource).not.toHaveBeenCalled();
  });

  it("allows grouped Videos / Sets when a provider declares video support", async () => {
    const searchSource = vi.fn(async () => ({ complete: true, results: [] }));
    const target = render({
      initialSources: ["youtube"],
      availableSources: [{ id: "youtube", name: "YouTube", enabled: true, kinds: ["track", "video"] }],
      searchSource,
    });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "video search"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const videos = [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Videos / Sets")!;
    expect(videos.disabled).toBe(false);
    videos.click();
    await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ sourceIds: ["youtube"], kinds: ["video", "set"] }));
  });

  it("hides track controls for playlist results and restores their saved filter on track searches", async () => {
    const longTrack = { id: "long", provider: "library", providerId: "long", sourceUrl: "local:long", artist: "Artist", title: "Long enough", durationSeconds: 120, favorite: false, provenance: {} };
    const shortTrack = { ...longTrack, id: "short", providerId: "short", sourceUrl: "local:short", title: "Too short", durationSeconds: 30 };
    const target = render({
      initialSources: ["spotify"],
      availableSources: [{ id: "spotify", name: "Spotify", enabled: true, kinds: ["playlist", "profile", "channel", "artist", "album", "track"] }],
      searchSource: async ({ kinds }: { kinds: string[] }) => kinds.includes("playlist")
        ? ({ complete: true, results: [{ id: "mix", kind: "playlist", sourceId: "spotify", title: "Saved mix", url: "https://open.spotify.com/playlist/mix" }] })
        : kinds.includes("track")
          ? ({ complete: true, results: [longTrack, shortTrack].map(track => ({ id: track.id, kind: "track", sourceId: "spotify", title: track.title, track })) })
          : ({ complete: true, results: [] }),
    });
    await settle();
    const minDuration = target.querySelector<HTMLInputElement>('[aria-label="Minimum loaded track duration"]')!;
    minDuration.value = "90";
    minDuration.dispatchEvent(new Event("input", { bubbles: true }));
    const playlists = [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Playlists")!;
    playlists.click();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "saved"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(target.querySelector(".track-controls")).toBeNull();
    expect(target.querySelector("#track-controls-unavailable")).toBeNull();
    expect(target.textContent).toContain("Saved mix");
    for (const label of ["Profiles", "Channels", "Artists", "Albums"]) {
      [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === label)!.click();
      await settle();
      expect(target.querySelector(".track-controls"), `${label} results hide track-only controls`).toBeNull();
    }

    const tracks = [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Tracks")!;
    tracks.click();
    await settle();
    expect(target.querySelector<HTMLFieldSetElement>(".track-controls")?.disabled).toBe(false);
    expect(target.querySelector<HTMLInputElement>('[aria-label="Minimum loaded track duration"]')?.value).toBe("90");
    expect(target.textContent).toContain("Long enough");
    expect(target.textContent).not.toContain("Too short");
  });

  it("searches connected Spotify playlists and opens the selected import", async () => {
    const row={id:"0123456789ABCDEFGHIJKL",kind:"playlist",sourceId:"spotify",title:"Selected playlist",url:"https://open.spotify.com/playlist/0123456789ABCDEFGHIJKL"};
    const searchSource=vi.fn(async()=>({sourceId:"spotify",complete:true,results:[row]}));
    const onRemoteResult=vi.fn();
    const target=render({initialSources:["spotify"],availableSources:[{id:"spotify",name:"Spotify",enabled:true,kinds:["track","playlist"]}],searchSource,onRemoteResult});
    await settle();
    const input=target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value="Selected";input.dispatchEvent(new Event("input",{bubbles:true}));
    [...target.querySelectorAll<HTMLButtonElement>('.filters button')].find(button=>button.textContent==="Playlists")!.click();
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true}));
    await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({sourceIds:["spotify"],kinds:["playlist"]}));
    target.querySelector<HTMLButtonElement>('.open-result')!.click();
    expect(onRemoteResult).toHaveBeenCalledWith(row);
  });

  it("opens Spotify artist results at their provider profile", async () => {
    const row = { id: "artist-id", kind: "artist", sourceId: "spotify", title: "Artist", url: "https://open.spotify.com/artist/artist-id" };
    const searchSource = vi.fn(async () => ({ complete: true, results: [row] }));
    const onRemoteResult = vi.fn();
    const target = render({ initialSources: ["spotify"], availableSources: [{ id: "spotify", name: "Spotify", enabled: true, kinds: ["artist"] }], searchSource, onRemoteResult });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "Artist"; input.dispatchEvent(new Event("input", { bubbles: true }));
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")!.click();
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const button = target.querySelector<HTMLButtonElement>(".open-result")!;
    expect(button.textContent).toContain("Explore artist");
    button.click();
    expect(onRemoteResult).toHaveBeenCalledWith(row);
  });

  it("shows each non-track title once inside its action target", async () => {
    const longName = "Bulgarian State Radio & Television Female Vocal Choir — Long credited name";
    const artist = { id: "mb-artist", kind: "artist", sourceId: "musicbrainz", title: longName, subtitle: "Choir · Bulgaria", data: { artist: { provider: "musicbrainz", id: "mb-artist" } } };
    const album = { id: "mb-album", kind: "album", sourceId: "musicbrainz", title: "Folk Songs", subtitle: "Choir", data: { release: { provider: "musicbrainz", id: "mb-release" }, title: "Folk Songs", tracks: [], sourceUrl: "https://musicbrainz.org/release/mb-release", credits: [] } };
    const onArtist = vi.fn(); const onAlbum = vi.fn(); const onRemoteResult = vi.fn();
    const searchSource = vi.fn(async () => ({ sourceId: "musicbrainz", complete: true, results: [artist, album] }));
    const target = render({ initialSources: ["musicbrainz"], availableSources: [
      { id: "musicbrainz", name: "MusicBrainz", enabled: true, kinds: ["track", "artist", "album"] },
    ], searchSource, onArtist, onAlbum, onRemoteResult });
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "Choir"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();

    const rows = [...target.querySelectorAll<HTMLElement>(".source-group .result-row")];
    expect(rows).toHaveLength(2);
    expect(rows[0]!.querySelector(".kind-badge")?.textContent).toBe("artist");
    const artistButton = rows[0]!.querySelector<HTMLButtonElement>(".open-result")!;
    expect(artistButton.textContent?.match(new RegExp(longName, "g"))).toHaveLength(1);
    expect(artistButton.getAttribute("aria-label")).toContain(longName);
    expect(rows[0]!.textContent).toContain("Choir · Bulgaria");
    artistButton.click();
    expect(onArtist).toHaveBeenCalledWith({ provider: "musicbrainz", id: "mb-artist" });

    const albumButton = rows[1]!.querySelector<HTMLButtonElement>(".open-result")!;
    expect(albumButton.textContent?.match(/Folk Songs/g)).toHaveLength(1);
    albumButton.click();
    expect(onAlbum).toHaveBeenCalledWith(album.data);
  });

  it("keeps playlist, profile, and channel titles as single direct action targets", async () => {
    const rows = [
      { id: "playlist-id", kind: "playlist", sourceId: "youtube", title: "Village playlist", url: "https://www.youtube.com/playlist?list=playlist-id" },
      { id: "profile-id", kind: "profile", sourceId: "spotify", title: "A long public profile name", url: "https://open.spotify.com/user/profile-id" },
      { id: "channel-id", kind: "channel", sourceId: "youtube", title: "Choir channel", url: "https://www.youtube.com/channel/channel-id" },
    ];
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ complete: true, results: rows.filter(row => row.sourceId === sourceIds[0]) }));
    const onRemoteResult = vi.fn();
    const target = render({
      initialSources: [],
      availableSources: [
        { id: "youtube", name: "YouTube", enabled: true, kinds: ["track", "playlist", "channel"] },
        { id: "spotify", name: "Spotify", enabled: true, kinds: ["track", "playlist", "profile"] },
      ],
      searchSource, onRemoteResult,
    });
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "choir"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".open-result")];
    expect(buttons).toHaveLength(3);
    for (const button of buttons) {
      const expectedTitle = button.getAttribute("title")!;
      expect(button.textContent?.match(new RegExp(expectedTitle, "g"))).toHaveLength(1);
      expect(button.getAttribute("aria-label")).toContain(expectedTitle);
      button.click();
    }
    expect(onRemoteResult.mock.calls.map(([row]) => row)).toEqual(expect.arrayContaining(rows));
  });

  it("collects additional pages without enabling queue continuation", async () => {
    const tracks = ["First", "Second"].map((title, i) => ({ id: `collect-${i}`, provider: "library", providerId: `collect-${i}`, sourceUrl: "", artist: "Artist", title, favorite: false, provenance: {} }));
    const searchSource = vi.fn(async ({ cursor }: { cursor?: string | null }) => ({ complete: !cursor ? false : true, nextCursor: cursor ? null : "page-2", results: [tracks[cursor ? 1 : 0]].map(track => ({ id: track.id, kind: "track", sourceId: "library", title: track.title, track })) }));
    const onPlay = vi.fn(); const target = render({ searchSource, onPlay }); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "collect"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    expect(target.querySelector<HTMLInputElement>(".continue-setting input")!.checked).toBe(false);
    target.querySelector<HTMLButtonElement>(".collect-more")!.click(); await settle();
    expect(searchSource).toHaveBeenCalledTimes(2);
    expect(target.textContent).toContain("Second");
    expect(target.querySelector<HTMLInputElement>(".continue-setting input")!.checked).toBe(false);
    target.querySelector<HTMLButtonElement>('[aria-label="Play Second"]')!.click(); await settle();
    expect(onPlay).toHaveBeenCalledWith(tracks[1]);
  });

  it("shows continuation when a provider page is complete but still has a cursor", async () => {
    const first = { id: "page-first", kind: "track", sourceId: "library", title: "First", track: { id: "page-first", provider: "library", providerId: "page-first", sourceUrl: "", artist: "Artist", title: "First", favorite: false, provenance: {} } };
    const second = { id: "page-second", kind: "track", sourceId: "library", title: "Second", track: { id: "page-second", provider: "library", providerId: "page-second", sourceUrl: "", artist: "Artist", title: "Second", favorite: false, provenance: {} } };
    const searchSource = vi.fn(async ({ cursor }: { cursor?: string | null }) => cursor
      ? { complete: true, results: [first, second] }
      : { complete: true, nextCursor: "page-two", results: [first] });
    const target = render({ searchSource }); await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "paged"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    expect(target.textContent).toContain("More results available");
    expect(target.querySelector<HTMLButtonElement>(".load-more")).not.toBeNull();
    target.querySelector<HTMLButtonElement>(".load-more")!.click(); await settle();
    expect(searchSource).toHaveBeenCalledTimes(2);
    expect(target.textContent).toContain("Complete");
    expect(target.textContent).not.toContain("More results available");
    expect(target.querySelector(".load-more")).toBeNull();
    expect(target.querySelectorAll("article.result-row")).toHaveLength(2);
    expect(target.textContent).toContain("First");
    expect(target.textContent).toContain("Second");
  });

  it("stops cursor cycles, retains collected rows and resets cursor history for a new search", async () => {
    const searchSource = vi.fn(async ({ cursor }: { cursor?: string | null }) => {
      const id = cursor || "first";
      const track = { id, provider: "library", providerId: id, sourceUrl: "", artist: "Artist", title: `Song ${id}`, favorite: false, provenance: {} };
      return { complete: false, nextCursor: cursor === "a" ? "b" : "a", results: [{ id, kind: "track", sourceId: "library", title: track.title, track }] };
    });
    const target = render({ searchSource }); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "cycle"; input.dispatchEvent(new Event("input", { bubbles: true }));
    const submit = () => target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    submit(); await settle(); target.querySelector<HTMLButtonElement>(".collect-more")!.click();
    for (let i=0;i<4;i++) await settle();
    expect(searchSource).toHaveBeenCalledTimes(3);
    expect(target.textContent).toContain("Song first"); expect(target.textContent).toContain("Song a"); expect(target.textContent).toContain("Song b");
    expect(target.textContent).toContain("Loaded results are incomplete");
    expect(target.querySelector(".load-more")).toBeNull();
    input.value = "fresh"; input.dispatchEvent(new Event("input", { bubbles: true })); submit(); await settle();
    expect(target.querySelector(".load-more")).not.toBeNull();
    expect(target.textContent).not.toContain("Loaded results are incomplete");
  });

  it("labels incomplete results without a continuation cursor honestly", async () => {
    const partialRow = { id: "partial", kind: "artist", sourceId: "library", title: "Partial result" };
    const searchSource = async ({ cursor }: { cursor?: string | null }) => cursor
      ? { complete: false, results: Array.from({ length: 10_001 }, (_, index) => ({ ...partialRow, id: `partial-${index}` })) }
      : { complete: false, nextCursor: "page-2", results: [] };
    const target = render({ searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "partial"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(target.querySelector<HTMLButtonElement>(".collect-more")).not.toBeNull();
    target.querySelector<HTMLButtonElement>(".collect-more")!.click();
    await settle();
    expect(target.textContent).toContain("Partial result");
    target.querySelector<HTMLButtonElement>(".collect-more")!.click();
    await settle();
    expect(target.textContent).toContain("Incomplete results; no continuation is available");
    expect(target.textContent).not.toContain("More results available");
    expect(target.querySelector(".load-more")).toBeNull();
  });

  it("continues collected tracks from the selected row when enabled", async () => {
    const tracks = ["Second", "First"].map((title, index) => ({ id: `t${index}`, provider: "library", providerId: `t${index}`, sourceUrl: `local:t${index}`, artist: "Artist", title, favorite: false, provenance: {} }));
    const searchSource = vi.fn(async ({ cursor }: { cursor?: string | null }) => cursor
      ? ({ complete: true, results: [{ id: "t2", kind: "track", sourceId: "library", title: "Third", track: { ...tracks[0], id: "t2", title: "Third" } }] })
      : ({ complete: true, nextCursor: "next", results: tracks.map(track => ({ id: track.id, kind: "track", sourceId: "library", title: track.title, track })) }));
    const onPlay = vi.fn();
    const target = render({ searchSource, onPlay });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "tracks"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(target.querySelector<HTMLInputElement>(".continue-setting input")?.checked).toBe(false);
    target.querySelector<HTMLInputElement>(".continue-setting input")!.click();
    await settle();
    expect(storage.get("burrata.discovery.continue-search.v1")).toBe("true");
    const sort = target.querySelector<HTMLSelectElement>("[aria-label='Sort loaded tracks']")!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true }));
    target.querySelector<HTMLButtonElement>(".collect-more")!.click();
    await settle();
    target.querySelector<HTMLButtonElement>("[aria-label='Play Second']")!.click();
    await settle();
    expect(onPlay).toHaveBeenCalledWith(tracks[0], [tracks[1], tracks[0], { ...tracks[0], id: "t2", title: "Third" }], 1);
  });

  it("keeps repeated same-reference search occurrences in descending source order", async () => {
    const repeated = { id: "repeat", provider: "library", providerId: "repeat", sourceUrl: "local:repeat", artist: "Artist", title: "Repeated", favorite: false, provenance: {} };
    const searchSource = vi.fn(async () => ({ complete: true, results: [
      { id: "occurrence-first", kind: "track", sourceId: "library", title: "Repeated", track: repeated },
      { id: "occurrence-second", kind: "track", sourceId: "library", title: "Repeated", track: repeated },
    ] }));
    const onPlay = vi.fn();
    const target = render({ searchSource, onPlay });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "repeated"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    target.querySelector<HTMLInputElement>(".continue-setting input")!.click();
    await settle();
    target.querySelector<HTMLButtonElement>("[aria-label='Toggle loaded track sort direction']")!.click();
    await settle();

    // The first rendered row is the second provider occurrence after reversal.
    target.querySelectorAll<HTMLButtonElement>(".track-title")[0]!.click();
    await settle();
    expect(onPlay).toHaveBeenCalledWith(repeated, [repeated, repeated], 0);
  });

  it("uses parsed mm:ss bounds and playable chapter duration in the displayed queue", async () => {
    const tracks = [
      { id: "long", provider: "library", providerId: "long", sourceUrl: "local:long", artist: "Artist", title: "Long", durationSeconds: 90, favorite: false, provenance: {} },
      { id: "chapter", provider: "library", providerId: "chapter", sourceUrl: "local:chapter", artist: "Artist", title: "Chapter", durationSeconds: 600, playbackRange: { parentTrackId: "parent", chapterId: "part", sourceUrl: "local:parent", startTime: 30, endTime: 75, parentTitle: "Parent" }, favorite: false, provenance: {} },
    ];
    const searchSource = vi.fn(async () => ({ complete: true, results: tracks.map(track => ({ id: track.id, kind: "track", sourceId: "library", title: track.title, track })) }));
    const onPlay = vi.fn();
    const target = render({ searchSource, onPlay });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "tracks"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    target.querySelector<HTMLInputElement>(".continue-setting input")!.click();
    const maximum = target.querySelector<HTMLInputElement>("[aria-label='Maximum loaded track duration']")!;
    maximum.value = "0:45"; maximum.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(target.querySelectorAll(".source-group .result-row")).toHaveLength(1);
    target.querySelector<HTMLButtonElement>('[aria-label="Play Chapter"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[1], [tracks[1]], 0);
  });

  it("keeps empty search messages separate from global empty-pane styling", async () => {
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [] }));
    const target = render({ searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "missing recording";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();

    const message = target.querySelector<HTMLElement>(".search-empty");
    expect(message?.textContent).toContain("No results from");
    expect(message?.classList.contains("empty")).toBe(false);
  });

  it("cancels a stale search response", async () => {
    let finish!: (value: { complete: boolean; results: { id: string; kind: string; sourceId: string; title: string }[] }) => void;
    const searchSource = vi.fn(() => new Promise<{ complete: boolean; results: { id: string; kind: string; sourceId: string; title: string }[] }>(resolve => { finish = resolve; }));
    const target = render({ searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>(".search-form input")!;
    input.value = "first"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(typeof finish).toBe("function"), { timeout: 1000 });
    target.querySelector<HTMLButtonElement>(".search-form button[type=button]")?.click();
    finish({ complete: true, results: [{ id: "stale", kind: "track", sourceId: "library", title: "Stale result" }] });
    await settle();
    expect(target.textContent).toContain("Cancelled");
    expect(target.textContent).not.toContain("Stale result");
    expect(target.textContent).not.toContain("No results from Library");
  });

  it("migrates the old source list to all online while excluding unsupported providers", async () => {
    storage.set("burrata.discovery.sources.v1", JSON.stringify(["library", "musicbrainz", "spotify", "beatport", "soundcloud"]));
    const target = render({ initialSources: undefined, availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "spotify", name: "Spotify (imported)", kinds: ["track"] },
      { id: "beatport", name: "Beatport", kinds: ["track"] },
      { id: "soundcloud", name: "SoundCloud", kinds: ["track", "set"], enabled: false },
    ] });
    await settle();
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Online"]')?.getAttribute("aria-pressed")).toBe("true");
    expect(target.querySelector<HTMLButtonElement>('[aria-label="My library"]')?.getAttribute("aria-pressed")).toBe("false");
    const select = target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    expect(select.value).toBe("all");
    expect([...select.options].map(option => option.value)).toEqual(["all", "musicbrainz", "discogs"]);
    expect(storage.get("burrata.discovery.search-scope.v2")).toBe(JSON.stringify({ mode: "online", sourceId: "all" }));
  });

  it("defaults to all enabled supported online sources", async () => {
    const target = render({ initialSources: undefined, availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video"] },
      { id: "soulseek", name: "Soulseek", kinds: ["track"], enabled: false },
    ] });
    await settle();
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Online"]')?.getAttribute("aria-pressed")).toBe("true");
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("all");
    expect([...target.querySelectorAll<HTMLOptionElement>('[aria-label="Search in"] option')].map(option => option.value))
      .toEqual(["all", "musicbrainz", "discogs", "youtube"]);
  });

  it("keeps My library controls focused on local tracks", async () => {
    const target = render({ initialSources: ["library"], availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["artist", "album", "track"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video"] },
    ] });
    await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')).toBeNull();
    expect(target.textContent).toContain("Saved tracks, imports and downloads");
    expect([...target.querySelectorAll<HTMLButtonElement>(".filters button")].map(button => button.textContent?.trim()))
      .toEqual(["All", "Tracks"]);
    expect(target.querySelector<HTMLButtonElement>('[aria-label="My library"]')?.getAttribute("aria-pressed")).toBe("true");
  });

  it("shows only online sources that support the active result type", async () => {
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [] }));
    const target = render({ initialSources: undefined, availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video"] },
      { id: "soundcloud", name: "SoundCloud", kinds: ["track", "set"] },
      { id: "soulseek", name: "Soulseek", kinds: ["track"] },
    ], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "artist"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Artists")!.click();
    await settle();
    expect(searchSource.mock.calls.slice(-2).map(([request]) => request.sourceIds[0])).toEqual(["musicbrainz", "discogs"]);
    expect(target.querySelector('[aria-label="YouTube results"]')).toBeNull();
    expect(target.querySelector('[aria-label="SoundCloud results"]')).toBeNull();
    expect(target.querySelector('[aria-label="Soulseek results"]')).toBeNull();
    expect(target.querySelector('[aria-label="MusicBrainz results"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Discogs results"]')).not.toBeNull();
  });

  it("searches library, all online sources, and one chosen source immediately on scope changes", async () => {
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({
      sourceId: sourceIds[0], complete: true,
      results: [{ id: `${sourceIds[0]}-hit`, kind: "track", sourceId: sourceIds[0], title: `${sourceIds[0]} hit` }],
    }));
    const target = render({ initialSources: ["library"], availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video", "artist"] },
      { id: "spotify", name: "Spotify (imported)", kinds: ["track"] },
      { id: "soulseek", name: "Soulseek", kinds: ["track"], enabled: false },
    ], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "artist title"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["library"] }));

    target.querySelector<HTMLButtonElement>('[aria-label="Online"]')!.click();
    await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["musicbrainz"] }));
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["discogs"] }));
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["youtube"] }));
    const select = target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    expect([...select.options].map(option => option.value)).toEqual(["all", "musicbrainz", "discogs", "youtube"]);
    expect(target.textContent).not.toContain("Spotify (imported)");
    expect(target.textContent).not.toContain("Soulseek");

    select.value = "youtube"; select.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["youtube"] }));
    expect(storage.get("burrata.discovery.search-scope.v2")).toBe(JSON.stringify({ mode: "online", sourceId: "youtube" }));
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Artists")!.click();
    await settle();
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ sourceIds: ["youtube"] }));
    target.querySelector<HTMLButtonElement>('[aria-label="My library"]')!.click();
    await settle();
    expect(target.querySelector<HTMLButtonElement>('.filters button[aria-pressed="true"]')?.textContent).toBe("Tracks");
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["library"], kinds: ["track"] }));
  });

  it("keeps a selected source through kind changes and explains unsupported kinds", async () => {
    const searchSource = vi.fn(async () => ({ complete: true, results: [] }));
    const target = render({ initialSources: ["discogs"], availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
    ], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "some artist"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(1);
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Tracks")!.click();
    await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("discogs");
    expect(searchSource).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("Discogs does not support tracks");
    expect(target.querySelector('[aria-label="Discogs results"]')).not.toBeNull();
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Albums")!.click();
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(2);
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ sourceIds: ["discogs"], kinds: ["album"] }));
  });

  it("cancels a Soulseek job whose start response arrives after a newer query", async () => {
    vi.useFakeTimers();
    const staleStart = deferred<unknown>();
    const searchSource = vi.fn(({ query }: { query: string }) => query === "old peer query"
      ? staleStart.promise
      : Promise.resolve({ sourceId: "soulseek", complete: false, progress: "searching", nextCursor: "new-session", results: [
        { id: "current-result", kind: "artist", sourceId: "soulseek", title: "Current result" },
      ] }));
    apiCall.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "poll_search_source") {
        expect(args).toEqual({ searchId: "new-session", sourceId: "soulseek" });
        return { sourceId: "soulseek", complete: true, nextCursor: null, results: [] };
      }
      if (command === "cancel_slskd_search") return true;
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({ initialSources: ["soulseek"], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    const submit = () => target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    input.value = "old peer query"; input.dispatchEvent(new Event("input", { bubbles: true })); submit(); await settle();
    expect(searchSource).toHaveBeenCalledTimes(1);

    input.value = "new peer query"; input.dispatchEvent(new Event("input", { bubbles: true })); submit(); await settle();
    await vi.advanceTimersByTimeAsync(900); await settle();
    expect(searchSource).toHaveBeenCalledTimes(2);

    staleStart.resolve({ sourceId: "soulseek", complete: false, progress: "searching", nextCursor: "old-session", results: [
      { id: "stale-result", kind: "artist", sourceId: "soulseek", title: "Stale result" },
    ] });
    await settle();
    expect(apiCall).toHaveBeenCalledWith("cancel_slskd_search", { searchId: "old-session" });
    expect(apiCall).not.toHaveBeenCalledWith("cancel_slskd_search", { searchId: "new-session" });
    expect(target.textContent).toContain("Current result");
    expect(target.textContent).not.toContain("Stale result");
    expect(target.querySelector(".load-more")).toBeNull();
  });

  it("polls a partial Soulseek search through page completion and loads its next cursor", async () => {
    const firstBatch = Array.from({ length: 50 }, (_, index) => ({
      id: `peer-${index + 1}`, kind: "track", sourceId: "soulseek", title: `Peer Track ${index + 1}`,
    }));
    const searchSource = vi.fn()
      .mockResolvedValueOnce({ sourceId: "soulseek", complete: false, progress: "searching", nextCursor: "slsk-search-1", results: [] })
      .mockResolvedValueOnce({ sourceId: "soulseek", complete: true, nextCursor: null, results: [
        { id: "peer-51", kind: "track", sourceId: "soulseek", title: "Peer Track 51" },
      ] });
    apiCall.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "poll_search_source") {
        expect(args).toEqual({ searchId: "slsk-search-1", sourceId: "soulseek" });
        return { sourceId: "soulseek", complete: true, nextCursor: "slsk-search-1@50", results: firstBatch };
      }
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({ initialSources: ["soulseek"], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "peer track"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("searching");

    await new Promise((resolve) => setTimeout(resolve, 950));
    await settle();
    expect(apiCall).toHaveBeenCalledWith("poll_search_source", { searchId: "slsk-search-1", sourceId: "soulseek" });
    expect(target.textContent).toContain("Peer Track 1");
    expect(target.textContent).not.toContain("Peer Track 50");
    expect(target.querySelectorAll(".virtual-result-item").length).toBeLessThan(40);
    expect(target.querySelector<HTMLButtonElement>(".load-more")).not.toBeNull();

    target.querySelector<HTMLButtonElement>(".load-more")!.click();
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(2);
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({
      query: "peer track", sourceIds: ["soulseek"], cursor: "slsk-search-1@50", limit: 30,
    }));
    expect(target.querySelectorAll(".virtual-result-item").length).toBeLessThan(40);
  });

  it("renders cached chapters as a disclosure and sends chapter playback through the callback", async () => {
    const chapter = { id: "c1", title: "First track", startTime: 0, endTime: 90, confidence: "high" };
    const track = { id: "video1", provider: "youtube", providerId: "video1", sourceUrl: "https://youtube.com/watch?v=1", artist: "DJ", title: "Live set", favorite: false, provenance: { audioCandidate: { id: "candidate1", provider: "youtube", url: "https://youtube.com/watch?v=1", title: "Live set", formats: [], permission: "unknown", confidence: "high", score: 1, reasons: [], official: false, chapters: [chapter] } } };
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [{ id: "video1", kind: "video", sourceId: "youtube", title: "Live set", url: track.sourceUrl, track }] }));
    const onPlayChapter = vi.fn();
    const target = render({ initialSources: ["youtube"], searchSource, onPlayChapter });
    await settle();
    const input = target.querySelector<HTMLInputElement>(".search-form input")!;
    input.value = "Live set"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const badge = target.querySelector<HTMLElement>(".chapter-badge.available")!;
    expect(badge.textContent).toContain("Chapters available · 1");
    expect(target.querySelectorAll(".chapter-row")).toHaveLength(0);
    badge.click();
    await new Promise(resolve => setTimeout(resolve, 0));
    await settle();
    expect(target.querySelectorAll(".chapter-row")).toHaveLength(1);
    target.querySelector<HTMLButtonElement>(".chapter-row button")!.click();
    expect(onPlayChapter).toHaveBeenCalledWith(track, track.provenance.audioCandidate, chapter);
    badge.click();
    await new Promise(resolve => setTimeout(resolve, 0));
    await settle();
    expect(target.querySelectorAll(".chapter-row")).toHaveLength(0);
    expect(target.querySelector(".chapter-badge.available")?.textContent).toContain("Chapters available · 1");
  });

  it("mounts inspected chapter actions only while expanded and keeps metadata cached when collapsed", async () => {
    const chapter = { id: "lazy-1", title: "Queued set section", startTime: 30, endTime: 90, confidence: "high" };
    const track = { id: "lazy-video", provider: "youtube", providerId: "lazy-video", sourceUrl: "https://youtube.com/watch?v=lazy", artist: "DJ", title: "Long set", favorite: false, provenance: {} };
    const candidate = { id: "candidate-lazy", provider: "youtube", url: track.sourceUrl, title: track.title, formats: [], permission: "unknown", confidence: "high", score: 1, reasons: [], official: false, chapters: [chapter] };
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [{ id: track.id, kind: "video", sourceId: "youtube", title: track.title, url: track.sourceUrl, track }] }));
    const enrichSource = vi.fn(async () => candidate);
    const onPlayChapter = vi.fn();
    const target = render({ initialSources: ["youtube"], searchSource, enrichSource, onPlayChapter });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "Long set"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const inspect = target.querySelector<HTMLButtonElement>(".chapter-badge.unknown")!;
    inspect.click();
    await settle();
    const badge = target.querySelector<HTMLElement>(".chapter-badge.available")!;
    expect(enrichSource).toHaveBeenCalledTimes(1);
    expect(target.querySelectorAll(".chapter-row")).toHaveLength(0);

    badge.click();
    await new Promise(resolve => setTimeout(resolve, 0));
    await settle();
    expect(target.querySelectorAll(".chapter-row")).toHaveLength(1);
    target.querySelector<HTMLButtonElement>(".chapter-row button")!.click();
    expect(onPlayChapter).toHaveBeenCalledWith(track, candidate, chapter);

    badge.click();
    await new Promise(resolve => setTimeout(resolve, 0));
    await settle();
    expect(target.querySelectorAll(".chapter-row")).toHaveLength(0);
    expect(enrichSource).toHaveBeenCalledTimes(1);
    expect(target.querySelector(".chapter-badge.available")?.textContent).toContain("Chapters available · 1");
  });

  it("does not advertise empty or unusable cached chapter lists", async () => {
    const candidate = (id: string, chapters: unknown[]) => ({
      id, provider: "soundcloud", url: `https://soundcloud.com/artist/${id}`, title: id,
      formats: [], permission: "unknown", confidence: "unknown", score: 0, reasons: [], official: false, chapters,
    });
    const emptyTrack = { id: "empty", provider: "soundcloud", providerId: "empty", sourceUrl: "https://soundcloud.com/artist/empty", artist: "Artist", title: "Empty chapters", favorite: false,
      provenance: { audioCandidate: candidate("empty", []) } };
    const invalidTrack = { id: "invalid", provider: "soundcloud", providerId: "invalid", sourceUrl: "https://soundcloud.com/artist/invalid", artist: "Artist", title: "Invalid chapters", favorite: false,
      provenance: { audioCandidate: candidate("invalid", [{ id: "bad", title: "No duration", startTime: 0, endTime: 0, confidence: "low" }]) } };
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [
      { id: "empty", kind: "video", sourceId: "soundcloud", title: emptyTrack.title, url: emptyTrack.sourceUrl, track: emptyTrack },
      { id: "invalid", kind: "video", sourceId: "soundcloud", title: invalidTrack.title, url: invalidTrack.sourceUrl, track: invalidTrack },
    ] }));
    const target = render({ initialSources: ["soundcloud"], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "chapters"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();

    expect(target.querySelectorAll(".chapter-badge.available")).toHaveLength(0);
    expect(target.querySelectorAll(".chapter-details")).toHaveLength(0);
    expect(target.querySelectorAll(".chapter-badge:not(.available)")).toHaveLength(2);
    expect(target.textContent).not.toContain("Chapters available · 0");
    expect(target.textContent).toContain("No chapters");
  });

  it("collapses provider groups accessibly while retaining pagination and excluding hidden results from continuation", async () => {
    const mb = { id: "mb-track", provider: "musicbrainz", providerId: "mb-track", sourceUrl: "https://musicbrainz.test/recording/mb-track", title: "MB Song", artist: "Artist", favorite: false, provenance: {} };
    const discogs = { id: "discogs-track", provider: "discogs", providerId: "discogs-track", sourceUrl: "https://discogs.test/release/discogs-track", title: "Discogs Song", artist: "Artist", favorite: false, provenance: {} };
    const searchSource = vi.fn(async ({ sourceIds, cursor }: { sourceIds: string[]; cursor?: string | null }) => {
      if (sourceIds[0] === "musicbrainz") return cursor
        ? { sourceId: "musicbrainz", complete: true, results: [{ id: "mb-later", kind: "track", sourceId: "musicbrainz", title: "MB Later", track: { ...mb, id: "mb-later", title: "MB Later" } }] }
        : { sourceId: "musicbrainz", complete: false, nextCursor: "page-2", results: [{ id: "mb-row", kind: "track", sourceId: "musicbrainz", title: mb.title, track: mb }] };
      return { sourceId: "discogs", complete: true, results: [{ id: "discogs-row", kind: "track", sourceId: "discogs", title: discogs.title, track: discogs }] };
    });
    const onPlay = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(SearchPanel, { target, props: {
      initialSources: [],
      availableSources: [
        { id: "musicbrainz", name: "MusicBrainz", enabled: true, kinds: ["track"] },
        { id: "discogs", name: "Discogs", enabled: true, kinds: ["track"] },
      ], searchSource, onPlay,
    } as never });
    destroy = () => unmount(instance);
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "song"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLFormElement>(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    target.querySelector<HTMLInputElement>(".continue-setting input")!.click(); await settle();
    const initialAutomation = instance.inspectAutomationSearchUi(0, 10) as { revision: string; total: number; rows: { occurrenceToken: string; sourceId: string }[] };
    expect(initialAutomation.total).toBe(2);

    const mbGroup = [...target.querySelectorAll<HTMLElement>(".source-group")].find(group => group.getAttribute("aria-label") === "MusicBrainz results")!;
    const toggle = mbGroup.querySelector<HTMLButtonElement>(".source-toggle")!;
    expect(toggle.tagName).toBe("BUTTON");
    expect(toggle.getAttribute("aria-controls")).toBeTruthy();
    toggle.focus();
    expect(document.activeElement).toBe(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    toggle.click(); await settle();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(instance.playAutomationSearchResult(initialAutomation.revision, initialAutomation.rows.find(row => row.sourceId === "musicbrainz")!.occurrenceToken, Number.MAX_SAFE_INTEGER)).toMatchObject({ status: "stale" });
    const collapsedAutomation = instance.inspectAutomationSearchUi(0, 10) as { revision: string; total: number; rows: { sourceId: string }[] };
    expect(collapsedAutomation.total).toBe(1);
    expect(collapsedAutomation.rows.map(row => row.sourceId)).toEqual(["discogs"]);
    expect(collapsedAutomation.revision).not.toBe(initialAutomation.revision);
    expect(mbGroup.querySelector<HTMLDivElement>("[id^='search-results-']")?.hidden).toBe(true);
    expect(mbGroup.querySelector(".result-row")?.textContent).toContain("MB Song");
    expect(mbGroup.querySelector<HTMLButtonElement>(".load-more")).not.toBeNull();

    const discogsGroup = [...target.querySelectorAll<HTMLElement>(".source-group")].find(group => group.getAttribute("aria-label") === "Discogs results")!;
    discogsGroup.querySelector<HTMLButtonElement>(".icon-action.play")!.click(); await settle();
    expect(onPlay).toHaveBeenCalledWith(discogs, [discogs], 0);

    mbGroup.querySelector<HTMLButtonElement>(".load-more")!.click(); await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ sourceIds: ["musicbrainz"], cursor: "page-2" }));
    expect(mbGroup.querySelector<HTMLDivElement>("[id^='search-results-']")?.hidden).toBe(true);
    toggle.click(); await settle();
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(mbGroup.textContent).toContain("MB Song");
    expect(mbGroup.textContent).toContain("MB Later");
    const restoredAutomation = instance.inspectAutomationSearchUi(0, 10) as { total: number; rows: { sourceId: string }[] };
    expect(restoredAutomation.total).toBe(3);
    expect(restoredAutomation.rows.map(row => row.sourceId)).toEqual(["musicbrainz", "musicbrainz", "discogs"]);
  });
});
