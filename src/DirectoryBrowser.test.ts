// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
// @ts-expect-error Match the browser runtime for reactive controlled props.
import { writable, fromStore } from "../node_modules/svelte/src/store/index-client.js";
import DirectoryBrowser from "./DirectoryBrowser.svelte";
import type { DirectoryBrowserSnapshot } from "./DirectoryBrowser.svelte";
import type { DirectoryAudioFile, DirectoryBrowsePage, DirectoryBrowserRoot, DirectoryScanJob } from "./lib/directory-browser";
import type { Track } from "./lib/types";

const root: DirectoryBrowserRoot = { id: "root-one", name: "Music", path: "/home/test/Music" };
const track: Track = { id: "recording-one", provider: "local", providerId: "hash-one", sourceUrl: "local:hash-one", artist: "Artist", title: "Track", favorite: false, localPath: "/home/test/Music/Track.flac", assetId: "asset-one", provenance: {} };
const file: DirectoryAudioFile = { path: track.localPath!, name: "Track.flac", assetId: "asset-one", track };
function browse(overrides: Partial<DirectoryBrowsePage> = {}): DirectoryBrowsePage {
  return { rootId: root.id, path: root.path, relativePath: "", breadcrumbs: [{ name: root.name, relativePath: "" }], directories: [], files: [file], total: 1, offset: 0, limit: 200, ...overrides };
}
let cleanup: (() => void) | undefined;
beforeEach(() => {
  document.body.innerHTML = "";
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }
function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, onPlayFile = vi.fn(), props: Record<string,unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(DirectoryBrowser, { target, props: { call, onPlayFile, ...props } });
  cleanup = () => unmount(instance);
  return { target, onPlayFile };
}
function renderWithSnapshots(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(DirectoryBrowser, { target, props: { call } });
  cleanup = () => unmount(instance);
  return { target, api: instance as unknown as { getSnapshot: () => DirectoryBrowserSnapshot; restoreSnapshot: (snapshot: DirectoryBrowserSnapshot) => Promise<void> } };
}

describe("DirectoryBrowser", () => {
  it("keeps sort fields enabled while summary is pending, then disables only absent values", async () => {
    const fields = [false, true, false, true, false, false, false, false, false, false] as const;
    const call=vi.fn(async(command:string)=>{
      if(command==="list_directory_browser_roots")return [root];
      if(command==="browse_directory_browser")return browse();
      if(command==="start_directory_browser_sort_availability")return {jobId:"summary-1",rootId:root.id,relativePath:"",state:"running",completed:0,total:1,fields:null,error:null};
      if(command==="get_directory_browser_sort_availability")return {jobId:"summary-1",rootId:root.id,relativePath:"",state:"completed",completed:1,total:1,fields,error:null};
      throw Error(`Unexpected ${command}`);
    });
    const {target}=render(call); await settle();
    const sort=target.querySelector<HTMLSelectElement>('[aria-label="Sort directory tracks"]')!;
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Checking sort fields");
    expect(sort.querySelector<HTMLOptionElement>('option[value="title"]')?.disabled).toBe(false);
    await new Promise(resolve=>setTimeout(resolve,10)); await settle();
    expect(sort.querySelector<HTMLOptionElement>('option[value="title"]')?.disabled).toBe(true);
    expect(sort.querySelector('option[value="title"]')?.textContent).toContain("unavailable");
    expect(sort.querySelector<HTMLOptionElement>('option[value="duration"]')?.disabled).toBe(false);
    expect(sort.querySelector<HTMLOptionElement>('option[value="filename"]')?.disabled).toBe(false);
  });

  it("cancels a summary that finishes starting after the view is destroyed", async () => {
    const started=deferred<unknown>();
    const call=vi.fn((command:string)=>{
      if(command==="list_directory_browser_roots")return Promise.resolve([root]);
      if(command==="browse_directory_browser")return Promise.resolve(browse());
      if(command==="start_directory_browser_sort_availability")return started.promise;
      if(command==="cancel_directory_browser_sort_availability")return Promise.resolve({});
      throw Error(`Unexpected ${command}`);
    });
    const {target}=render(call); await settle();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Checking sort fields");
    cleanup?.(); cleanup=undefined;
    started.resolve({jobId:"late-summary",rootId:root.id,relativePath:"",state:"running",completed:0,total:1});
    await settle();
    expect(call).toHaveBeenCalledWith("cancel_directory_browser_sort_availability",{jobId:"late-summary"});
  });

  it("uses availability returned by the global filtered browse without starting a duplicate summary", async () => {
    const fields = [false, false, false, true, false, false, false, false, false, false] as const;
    const call=vi.fn(async(command:string)=>{
      if(command==="list_directory_browser_roots")return [root];
      if(command==="browse_directory_browser")return browse({sortAvailability:fields});
      throw Error(`Unexpected ${command}`);
    });
    const {target}=render(call); await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort directory tracks"] option[value="duration"]')?.disabled).toBe(false);
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort directory tracks"] option[value="title"]')?.disabled).toBe(true);
    expect(call).not.toHaveBeenCalledWith("start_directory_browser_sort_availability", expect.anything());
  });

  it("keeps sort options enabled and reports a failed summary instead of treating it as unavailable", async () => {
    const call=vi.fn(async(command:string)=>{
      if(command==="list_directory_browser_roots")return [root];
      if(command==="browse_directory_browser")return browse();
      if(command==="start_directory_browser_sort_availability")return {jobId:"summary-fail",rootId:root.id,relativePath:"",state:"failed",completed:0,total:1,fields:null,error:"metadata index unavailable"};
      throw Error(`Unexpected ${command}`);
    });
    const {target}=render(call); await settle();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("metadata index unavailable");
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort directory tracks"] option[value="title"]')?.disabled).toBe(false);
  });
  it("uses sidebar-controlled locations and compact breadcrumbs without duplicate navigation", async () => {
    const state=writable({location:{rootId:root.id,relativePath:"Jazz"},revision:0});const model=fromStore(state);const onLocationChange=vi.fn();
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="list_directory_browser_roots")return [root];
      if(command==="browse_directory_browser")return browse({relativePath:String(args?.relativePath),breadcrumbs:args?.relativePath?[{name:"Music",relativePath:""},{name:"Jazz",relativePath:"Jazz"}]:[{name:"Music",relativePath:""}],directories:[{name:"Subfolder",relativePath:"Jazz/Subfolder"}]});
      throw Error(`Unexpected ${command}`);
    });
    const target=document.createElement("div");document.body.append(target);
    const instance=mount(DirectoryBrowser,{target,props:{call,onLocationChange,get location(){return model.current.location},get rootRegistryRevision(){return model.current.revision}}});cleanup=()=>unmount(instance);await settle();
    expect(call).toHaveBeenCalledWith("browse_directory_browser",expect.objectContaining({rootId:root.id,relativePath:"Jazz",offset:0,limit:200,query:"",sortField:"filename",sortDirection:"asc"}));
    expect(target.querySelector('[aria-label="Choose music folder"]')).toBeNull();
    expect(target.querySelector('[aria-label="Subfolders"]')).toBeNull();
    target.querySelector<HTMLButtonElement>('[aria-label="Folder path"] button')!.click();await settle();
    expect(onLocationChange).toHaveBeenCalledWith({rootId:root.id,relativePath:""}, expect.objectContaining({reason:"user",previousSnapshot:expect.objectContaining({location:{rootId:root.id,relativePath:"Jazz"}})}));
    state.set({location:{rootId:root.id,relativePath:"Jazz"},revision:1});await settle();
    expect(call.mock.calls.filter(([command])=>command==="list_directory_browser_roots")).toHaveLength(2);
    expect(target.querySelector('[aria-current="location"]')?.textContent).toBe("Jazz");
  });

  it("adds from the empty fallback and notifies the shared registry and location",async()=>{
    const onRootsChanged=vi.fn(),onLocationChange=vi.fn();
    const call=vi.fn(async(command:string)=>{
      if(command==="list_directory_browser_roots")return [];
      if(command==="choose_directory_browser_root")return {path:root.path};
      if(command==="add_directory_browser_root")return root;
      if(command==="browse_directory_browser")return browse();
      throw Error(`Unexpected ${command}`);
    });
    const {target}=render(call,vi.fn(),{onRootsChanged,onLocationChange});await settle();
    target.querySelector<HTMLButtonElement>(".directory-empty-state button")!.click();await settle();
    expect(onRootsChanged).toHaveBeenCalledTimes(1);expect(onLocationChange).toHaveBeenCalledWith({rootId:root.id,relativePath:""}, expect.objectContaining({reason:"user",previousSnapshot:expect.objectContaining({location:null})}));
    expect(target.querySelector('[aria-label="Play Track"]')).not.toBeNull();
  });

  it("uses playlist rows for audio while preserving unavailable-file rows in the mixed directory order", async () => {
    const unavailable: DirectoryAudioFile = { path: "/home/test/Music/Unindexed.flac", name: "Unindexed.flac", track: null };
    const call = vi.fn(async (command: string) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse({ files: [file, unavailable], total: 2 });
      throw Error(`Unexpected ${command}`);
    });
    const { target } = render(call); await settle();

    expect(target.querySelector(".track-list-viewport")?.classList.contains("playlist")).toBe(true);
    const rows = [...target.querySelectorAll<HTMLLIElement>(".shared-track-list > li")];
    expect(rows).toHaveLength(2);
    expect(rows[0]?.querySelector(".position-play")?.getAttribute("aria-label")).toBe("Play Track");
    expect(rows[0]?.querySelector(".track-position")?.textContent).toBe("1");
    expect(rows[1]?.classList.contains("track-status-row")).toBe(true);
    expect(rows[1]?.getAttribute("role")).toBe("status");
    expect(rows[1]?.textContent).toContain("Unindexed.flac");
    expect(rows[1]?.querySelector(".position-play")).toBeNull();
  });

  it("ignores a stale directory response after an external location change",async()=>{
    let finish!:(page:DirectoryBrowsePage)=>void;
    const state=writable({rootId:root.id,relativePath:"Old"});const model=fromStore(state);
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="list_directory_browser_roots")return [root];
      if(command==="browse_directory_browser")return args?.relativePath==="Old"?new Promise<DirectoryBrowsePage>(resolve=>finish=resolve):browse({relativePath:"New",files:[{...file,name:"New.flac",path:root.path+"/New.flac"}]});
      throw Error(`Unexpected ${command}`);
    });
    const target=document.createElement("div");document.body.append(target);const instance=mount(DirectoryBrowser,{target,props:{call,get location(){return model.current}}});cleanup=()=>unmount(instance);await settle();
    state.set({rootId:root.id,relativePath:"New"});await settle();
    finish(browse({relativePath:"Old"}));await settle();
    expect(target.querySelector('[aria-label="Play Track"]')).not.toBeNull();expect(target.textContent).toContain("New.flac");
    expect(call).toHaveBeenCalledWith("browse_directory_browser",expect.objectContaining({rootId:root.id,relativePath:"New",offset:0,limit:200,query:"",sortField:"filename",sortDirection:"asc"}));
  });

  it("cancels an older same-location snapshot restore before it can append or scroll", async () => {
    const pending = deferred<DirectoryBrowsePage>();
    let matchingCalls = 0;
    const newerFile = { ...file, name: "New snapshot.flac", path: "/home/test/Music/New snapshot.flac" };
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") {
        if (args?.query === "same query") {
          matchingCalls++;
          return matchingCalls === 1 ? pending.promise : browse({ files: [newerFile], total: 2 });
        }
        return browse();
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, api } = renderWithSnapshots(call); await settle();
    const base = api.getSnapshot();
    const oldRestore = api.restoreSnapshot({ ...base, query: "same query", loadedCount: 2, scrollTop: 12 });
    await settle();
    await api.restoreSnapshot({ ...base, query: "same query", loadedCount: 1, scrollTop: 91 });
    const viewport = target.querySelector<HTMLElement>(".track-list-viewport")!;
    viewport.scrollTop = 37;
    pending.resolve(browse({ files: [{ ...file, name: "Old snapshot.flac", path: "/home/test/Music/Old snapshot.flac" }], total: 2 }));
    await oldRestore; await settle();
    expect(target.textContent).toContain("New snapshot.flac");
    expect(target.textContent).not.toContain("Old snapshot.flac");
    expect(call).not.toHaveBeenCalledWith("browse_directory_browser", expect.objectContaining({ query: "same query", offset: 1 }));
    expect(viewport.scrollTop).toBe(37);
  });

  it("snapshots pending filter controls without old loaded rows or scroll and cancels the debounce on restore", async () => {
    vi.useFakeTimers();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse({ files: [{ ...file, name: `${String(args?.query || "initial")}.flac` }] });
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, api } = renderWithSnapshots(call); await settle();
    const viewport = target.querySelector<HTMLElement>(".track-list-viewport")!;
    viewport.scrollTop = 73;
    const input = target.querySelector<HTMLInputElement>('[aria-label="Filter directory tracks"]')!;
    input.value = "pending query"; input.dispatchEvent(new Event("input", { bubbles: true }));
    const snapshot = api.getSnapshot();
    expect(snapshot.query).toBe("pending query");
    expect(snapshot.loadedCount).toBe(0);
    expect(snapshot.scrollTop).toBe(0);
    const beforeRestore = call.mock.calls.filter(([command]) => command === "browse_directory_browser").length;
    await api.restoreSnapshot(snapshot); await settle();
    await vi.advanceTimersByTimeAsync(250); await settle();
    expect(call.mock.calls.filter(([command]) => command === "browse_directory_browser")).toHaveLength(beforeRestore + 1);
    expect(target.textContent).toContain("pending query.flac");
  });

  it("invalidates a restoring request immediately when the user edits the filter", async () => {
    vi.useFakeTimers();
    const pending = deferred<DirectoryBrowsePage>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return args?.query === "slow query" ? pending.promise : browse();
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, api } = renderWithSnapshots(call); await settle();
    const snapshot = { ...api.getSnapshot(), query: "slow query", loadedCount: 1, scrollTop: 44 };
    const restoring = api.restoreSnapshot(snapshot); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Filter directory tracks"]')!;
    input.value = "new filter"; input.dispatchEvent(new Event("input", { bubbles: true }));
    const pendingSnapshot = api.getSnapshot();
    expect(pendingSnapshot.query).toBe("new filter");
    expect(pendingSnapshot.loadedCount).toBe(0);
    expect(pendingSnapshot.scrollTop).toBe(0);
    pending.resolve(browse({ files: [{ ...file, name: "Stale old restore.flac" }] }));
    await restoring; await settle();
    expect(target.textContent).not.toContain("Stale old restore.flac");
    expect(call).not.toHaveBeenCalledWith("browse_directory_browser", expect.objectContaining({ query: "new filter" }));
  });

  it("keeps a root scan while browsing its children and cancels only when changing roots",async()=>{
    vi.useFakeTimers();
    const other={...root,id:"root-two",name:"Other"};
    const running:DirectoryScanJob={jobId:"root-scan",rootId:root.id,state:"running",completed:1,indexed:1,failed:0};
    const state=writable({rootId:root.id,relativePath:""});const model=fromStore(state);
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="list_directory_browser_roots")return [root,other];
      if(command==="browse_directory_browser")return browse({rootId:String(args?.rootId),relativePath:String(args?.relativePath)});
      if(command==="start_directory_browser_scan"||command==="get_directory_browser_scan")return running;
      if(command==="cancel_directory_browser_scan")return {...running,state:"cancelled"};
      throw Error(`Unexpected ${command}`);
    });
    const target=document.createElement("div");document.body.append(target);const instance=mount(DirectoryBrowser,{target,props:{call,get location(){return model.current}}});cleanup=()=>unmount(instance);await settle();
    target.querySelector<HTMLButtonElement>(".directory-toolbar button:last-child")!.click();await settle();
    state.set({rootId:root.id,relativePath:"Jazz"});await settle();
    expect(call.mock.calls.filter(([command])=>command==="cancel_directory_browser_scan")).toHaveLength(0);
    expect(target.textContent).toContain("Scanning files");
    state.set({rootId:other.id,relativePath:"Live"});await settle();
    expect(call).toHaveBeenCalledWith("cancel_directory_browser_scan",{jobId:running.jobId});
    expect(call).toHaveBeenCalledWith("browse_directory_browser",expect.objectContaining({rootId:other.id,relativePath:"Live",offset:0,limit:200,query:"",sortField:"filename",sortDirection:"asc"}));
    expect(target.textContent).not.toContain("Scanning files");
  });

  it("pages files by path occurrence and passes the exact indexed asset to playback", async () => {
    const duplicate = { ...file, path: "/home/test/Music/Copy/Track.flac", name: "Copy/Track.flac", assetId: "asset-two", track: { ...track, assetId: "asset-two", localPath: "/home/test/Music/Copy/Track.flac" } };
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return Number(args?.offset) === 0
        ? browse({ files: [file], total: 2 })
        : browse({ files: [duplicate], total: 2, offset: 1 });
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, onPlayFile } = render(call); await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Play Track"]')!.click();
    expect(onPlayFile).toHaveBeenCalledWith(file, track);
    target.querySelector<HTMLButtonElement>(".directory-load-more button")!.click(); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(2);
    target.querySelectorAll<HTMLButtonElement>(".shared-track-list .track-title")[1]!.click();
    expect(onPlayFile).toHaveBeenLastCalledWith(duplicate, duplicate.track);
    expect(call).toHaveBeenCalledWith("browse_directory_browser", expect.objectContaining({ rootId: root.id, relativePath: "", offset: 1, limit: 200, sortField: "filename", sortDirection: "asc" }));
  });

  it("opens an indexed local recording's album from its album name", async () => {
    const albumTrack = { ...track, album: "Local Album" };
    const albumFile = { ...file, track: albumTrack };
    const call = vi.fn(async (command: string) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse({ files: [albumFile] });
      throw new Error(`Unexpected command ${command}`);
    });
    const onTrackAlbum = vi.fn();
    const { target } = render(call, vi.fn(), { onTrackAlbum }); await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Open album Local Album"]')!.click();
    expect(onTrackAlbum).toHaveBeenCalledWith(albumTrack);
  });

  it("keeps unindexed files visible but unavailable for playback until scanning indexes them", async () => {
    const unindexed = { path: "/home/test/Music/new.flac", name: "new.flac", track: null };
    const call = vi.fn(async (command: string) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse({ files: [unindexed] });
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, onPlayFile } = render(call); await settle();
    expect(target.textContent).toContain("Metadata unavailable");
    expect(target.querySelector(".track-status-row button")).toBeNull();
    expect(onPlayFile).not.toHaveBeenCalled();
  });

  it("uses global directory query, sort, and fractional duration bounds before paging", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse({ files: [file], total: 17, offset: Number(args?.offset ?? 0) });
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    vi.useFakeTimers();
    const query = target.querySelector<HTMLInputElement>('[aria-label="Filter directory tracks"]')!;
    query.value = "live set"; query.dispatchEvent(new Event("input", { bubbles: true }));
    const minimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum directory track duration"]')!;
    minimum.value = "1:02.5"; minimum.dispatchEvent(new Event("input", { bubbles: true }));
    const maximum = target.querySelector<HTMLInputElement>('[aria-label="Maximum directory track duration"]')!;
    maximum.value = "62.9"; maximum.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(250); await settle();
    expect(call).toHaveBeenCalledWith("browse_directory_browser", expect.objectContaining({
      rootId: root.id, relativePath: "", offset: 0, limit: 200, query: "live set", sortField: "filename", sortDirection: "asc", durationMin: 62, durationMax: 62,
    }));
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort directory tracks"]')!;
    sort.value = "artist"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(call).toHaveBeenCalledWith("browse_directory_browser", expect.objectContaining({ sortField: "artist", offset: 0, query: "live set", durationMin: 62, durationMax: 62 }));
    target.querySelector<HTMLButtonElement>('[aria-label="Toggle directory sort direction"]')!.click(); await settle();
    expect(call).toHaveBeenCalledWith("browse_directory_browser", expect.objectContaining({ sortDirection: "desc", offset: 0 }));
    expect(target.textContent).toContain("1 of 17 files");
    const previousBrowseCount = call.mock.calls.filter(([command]) => command === "browse_directory_browser").length;
    minimum.value = "not a duration"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain("Enter duration in seconds or mm:ss.");
    expect(call.mock.calls.filter(([command]) => command === "browse_directory_browser")).toHaveLength(previousBrowseCount);
  });

  it("uses shared track actions while keeping repeated file paths as separate occurrences", async () => {
    const duplicate = { ...file, path: "/home/test/Music/Copy/Track.flac", name: "Copy/Track.flac" };
    const unindexed = { path: "/home/test/Music/New.flac", name: "New.flac", track: null };
    const call = vi.fn(async (command: string) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse({ files: [file, unindexed, duplicate], total: 3 });
      throw new Error(`Unexpected command ${command}`);
    });
    const onPlayFile = vi.fn(); const onFavorite = vi.fn(); const onAddSelectedToPlaylist = vi.fn();
    const { target } = render(call, onPlayFile, { onFavorite, onAddSelectedToPlaylist }); await settle();
    const rows = [...target.querySelectorAll(".shared-track-list > li")];
    expect(rows).toHaveLength(3);
    expect(rows[0]?.textContent).toContain("Track");
    expect(rows[1]?.textContent).toContain("New.flac");
    expect(rows[1]?.textContent).toContain("Metadata unavailable");
    expect(rows[1]?.querySelector("button")).toBeNull();
    expect(rows[2]?.textContent).toContain("Track");
    expect(target.querySelectorAll('.shared-track-list button[aria-label="Add Track to favorites"]')).toHaveLength(2);
    expect(target.querySelector('[aria-label="Filter directory tracks"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Sort directory tracks"]')).not.toBeNull();
    expect(target.querySelector(".select-track")).toBeNull();
    target.querySelector<HTMLButtonElement>(".start-selection")!.click();
    await settle();
    const checkboxes = target.querySelectorAll<HTMLInputElement>('[aria-label="Select Track for playlist"]');
    checkboxes[0]!.click(); await settle();
    target.querySelectorAll<HTMLInputElement>('[aria-label="Select Track for playlist"]')[1]!.click();
    await settle();
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([track, duplicate.track]);
    target.querySelectorAll<HTMLButtonElement>(".shared-track-list .track-title")[1]!.click();
    expect(onPlayFile).toHaveBeenCalledWith(duplicate, duplicate.track);
  });

  it("reports scan progress and allows cancellation without claiming files were removed", async () => {
    const calls: string[] = [];
    const running: DirectoryScanJob = { jobId: "scan-one", rootId: root.id, state: "running", completed: 3, indexed: 2, failed: 1, total: 20 };
    const call = vi.fn(async (command: string) => {
      calls.push(command);
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse();
      if (command === "start_directory_browser_scan") return running;
      if (command === "get_directory_browser_scan") return running;
      if (command === "cancel_directory_browser_scan") return { ...running, state: "cancelled" };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".directory-toolbar button:last-child")!.click(); await settle();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("3 of 20 checked");
    target.querySelector<HTMLButtonElement>(".directory-scan-status button")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("cancel_directory_browser_scan", { jobId: running.jobId });
    expect(target.textContent).toContain("Any files already indexed remain available");
    expect(calls).not.toContain("delete_directory_contents");
  });

  it("waits for the worker's terminal cancellation status before reporting cancellation", async () => {
    vi.useFakeTimers();
    let cancellationObserved = false;
    const running: DirectoryScanJob = { jobId: "scan-delayed", rootId: root.id, state: "running", completed: 1, indexed: 1, failed: 0 };
    const call = vi.fn(async (command: string) => {
      if (command === "list_directory_browser_roots") return [root];
      if (command === "browse_directory_browser") return browse();
      if (command === "start_directory_browser_scan") return running;
      if (command === "get_directory_browser_scan") return cancellationObserved ? { ...running, state: "cancelled" } : running;
      if (command === "cancel_directory_browser_scan") { cancellationObserved = true; return running; }
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".directory-toolbar button:last-child")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".directory-scan-status button")!.click(); await settle();
    expect(target.textContent).not.toContain("Directory scan canceled.");
    await vi.advanceTimersByTimeAsync(500); await settle();
    expect(target.textContent).toContain("Directory scan canceled. Any files already indexed remain available.");
  });
});
