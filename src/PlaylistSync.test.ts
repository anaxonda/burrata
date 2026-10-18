// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlaylistSync from "./PlaylistSync.svelte";
import PlaylistSyncTestHost from "./PlaylistSyncTestHost.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, enabled = true, onApplied = vi.fn()) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PlaylistSync, { target, props: { bindingId: "binding-1", enabled, call, onApplied } });
  destroy = () => unmount(instance);
  return { target, onApplied };
}
function renderHost(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PlaylistSyncTestHost, { target, props: { call } });
  destroy = () => unmount(instance);
  return target;
}
function preview(bindingId: string, summary = bindingId) {
  return {
    previewId: `preview-${bindingId}`, bindingId, provider: "spotify", playlistId: `playlist-${bindingId}`, localCollectionId: `collection-${bindingId}`,
    baselineRevision: "local-revision-1", remoteRevision: "remote-revision-1",
    canEdit: true, writeSyncEnabled: true, conflict: false, summary,
    changes: { add: [{ trackUri: `spotify:track:${bindingId}` }], remove: [], move: [], unresolved: [] },
  };
}

describe("PlaylistSync", () => {
  it("discards late recording matches after switching the source playlist", async () => {
    let resolveMatches!: (value:unknown)=>void;
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="get_remote_playlist_sync_preview")return {...preview(String(args?.bindingId)),changes:{add:[],remove:[],move:[],unresolved:[{localTrackId:"local-1",reason:"Unresolved"}]}};
      if(command==="search_remote_playlist_writeback_candidates")return new Promise(resolve=>{resolveMatches=resolve;});
      throw Error(`Unexpected ${command}`);
    });
    const target=renderHost(call);await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click();await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Find matching recording")!.click();await settle();
    target.querySelector<HTMLButtonElement>(".switch-binding")!.click();await settle();
    resolveMatches({bindingId:"binding-1",localTrackId:"local-1",provider:"spotify",candidates:[{candidateToken:"late",title:"Old recording",artist:"Old artist",url:""}]});await settle();
    expect(target.textContent).not.toContain("Old recording");
    expect(target.querySelector(".sync-apply")).toBeNull();
  });
  it("requires an explicit recording choice and regenerates the full preview", async () => {
    const call = vi.fn(async (command: string, args?: Record<string,unknown>) => {
      if (command === "get_remote_playlist_sync_preview") {
        const result = preview("binding-1");
        return args?.reviewedMatches ? result : {...result,changes:{add:[],remove:[],move:[],unresolved:[{localTrackId:"local-1",title:"Song",artist:"Artist",reason:"No verified Spotify ID"}]}};
      }
      if (command === "search_remote_playlist_writeback_candidates") return {bindingId:"binding-1",localTrackId:"local-1",provider:"spotify",candidates:[{candidateToken:"opaque-token",title:"Song",artist:"Artist",version:"Main Version",durationSeconds:281,url:"https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA"}]};
      throw Error(`Unexpected ${command}`);
    });
    const {target}=render(call);await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click();await settle();
    expect(target.querySelector(".sync-apply")).toBeNull();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Find matching recording")!.click();await settle();
    expect(target.textContent).toContain("Main Version · 4:41");
    expect(target.querySelector(".sync-apply")).toBeNull();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Use this recording")!.click();await settle();
    expect(call).toHaveBeenLastCalledWith("get_remote_playlist_sync_preview",{bindingId:"binding-1",reviewedMatches:[{localTrackId:"local-1",candidateToken:"opaque-token"}]});
    expect(target.querySelector(".sync-apply")).not.toBeNull();
    expect(call.mock.calls.some(([command])=>command==="apply_remote_playlist_sync_preview")).toBe(false);
    expect(target.querySelector('[aria-label="Chosen recordings"]')?.textContent).toContain("Song · Artist · Main Version · 4:41");
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Clear matches and refresh")!.click();await settle();
    expect(call).toHaveBeenLastCalledWith("get_remote_playlist_sync_preview",{bindingId:"binding-1"});
    expect(target.querySelector(".sync-apply")).toBeNull();
  });

  it("rejects candidate responses scoped to another recording", async () => {
    const call=vi.fn(async(command:string)=>{
      if(command==="get_remote_playlist_sync_preview")return {...preview("binding-1"),changes:{add:[],remove:[],move:[],unresolved:[{localTrackId:"local-1",reason:"Unresolved"}]}};
      return {bindingId:"binding-1",localTrackId:"other-track",provider:"spotify",candidates:[{candidateToken:"wrong",title:"Wrong",artist:"Wrong",url:""}]};
    });
    const {target}=render(call);await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click();await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Find matching recording")!.click();await settle();
    expect(target.textContent).toContain("different recording");
    expect([...target.querySelectorAll("button")].some(b=>b.textContent==="Use this recording")).toBe(false);
  });
  it("reviews every add, remove, and move before applying the full backend preview", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_remote_playlist_sync_preview") return {
        previewId: "preview-1", bindingId: "binding-1", provider: "spotify", playlistId: "playlist-1", localCollectionId: "collection-1",
        baselineRevision: "local-revision-1", remoteRevision: "remote-revision-1",
        canEdit: true, writeSyncEnabled: true, conflict: false, summary: "1 addition, 1 removal, 1 move",
        changes: { add: [{ trackUri: "spotify:track:new" }], remove: [{ entryIds: ["old-entry"] }], move: [{ entryIds: ["moving-entry"], beforeEntryId: "target-entry" }], unresolved: [] },
      };
      if (command === "apply_remote_playlist_sync_preview") return { applied: true, summary: "Spotify playlist synced." };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, onApplied } = render(call); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Sync")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("get_remote_playlist_sync_preview", { bindingId: "binding-1" });
    expect(target.textContent).toContain("spotify:track:new");
    expect(target.textContent).toContain("old-entry");
    expect(target.textContent).toContain("moving-entry → target-entry");
    expect(target.querySelector(".sync-apply")?.textContent).toContain("Apply changes");
    target.querySelector<HTMLButtonElement>(".sync-apply")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("apply_remote_playlist_sync_preview", { previewId: "preview-1" });
    expect(onApplied).toHaveBeenCalledTimes(1);
  });

  it("blocks unresolved non-Spotify items and never guesses or applies a partial plan", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_remote_playlist_sync_preview") return {
        previewId: "preview-unresolved", bindingId: "binding-1", provider: "spotify", playlistId: "playlist-1", localCollectionId: "collection-1",
        canEdit: true, writeSyncEnabled: true, conflict: false,
        changes: { add: [], remove: [], move: [], unresolved: [{ title: "YouTube item", reason: "no verified Spotify track ID" }] },
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(target.textContent).toContain("unavailable or unresolved on Spotify");
    expect(target.textContent).toContain("nothing will be guessed or skipped");
    expect(target.querySelector(".sync-apply")).toBeNull();
    expect(call).not.toHaveBeenCalledWith("apply_remote_playlist_sync_preview", expect.anything());
  });

  it("hides itself when write sync is disabled", async () => {
    const call = vi.fn(async () => { throw new Error("No calls expected"); });
    const { target } = render(call, false); await settle();
    expect(target.querySelector(".playlist-sync")).toBeNull();
    expect(call).not.toHaveBeenCalled();
  });

  it("blocks apply when the backend says write sync was turned off", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_remote_playlist_sync_preview") return {
        previewId: "preview-off", bindingId: "binding-1", provider: "spotify", playlistId: "playlist-1", localCollectionId: "collection-1",
        canEdit: true, writeSyncEnabled: false, conflict: false,
        changes: { add: [{ trackUri: "spotify:track:new" }], remove: [], move: [], unresolved: [] },
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(target.textContent).toContain("Spotify write sync is turned off");
    expect(target.querySelector(".sync-apply")).toBeNull();
  });

  it("blocks YouTube writes when its explicit write opt-in is disabled", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_remote_playlist_sync_preview") return {
        previewId: "preview-youtube", bindingId: "binding-1", provider: "youtube", playlistId: "playlist-1", localCollectionId: "collection-1",
        baselineRevision: "local-revision-1", remoteRevision: "remote-revision-1",
        canEdit: true, writeSyncEnabled: false, conflict: false,
        changes: { add: [{ trackUri: "youtube:video:abcdefghijk" }], remove: [], move: [], unresolved: [] },
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(target.textContent).toContain("Add to YouTube");
    expect(target.textContent).toContain("YouTube write sync is turned off");
    expect(target.querySelector(".sync-apply")).toBeNull();
    expect(call).not.toHaveBeenCalledWith("apply_remote_playlist_sync_preview", expect.anything());
  });

  it("allows one YouTube apply only for an explicitly enabled, complete reviewed preview", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_remote_playlist_sync_preview") return {
        ...preview("binding-1"), provider: "youtube", playlistId: "playlist-1",
        baselineRevision: "local-revision-1", remoteRevision: "remote-revision-1",
        writeSyncEnabled: true, changes: { add: [{ trackUri: "youtube:video:abcdefghijk" }], remove: [], move: [], unresolved: [] },
      };
      if (command === "apply_remote_playlist_sync_preview") return { applied: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, onApplied } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(target.textContent).toContain("experimental private interface");
    const apply = target.querySelector<HTMLButtonElement>(".sync-apply")!;
    expect(apply).not.toBeNull();
    apply.click(); await settle();
    expect(call).toHaveBeenCalledTimes(2);
    expect(call).toHaveBeenCalledWith("apply_remote_playlist_sync_preview", { previewId: "preview-binding-1" });
    expect(onApplied).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("YouTube playlist synced.");
  });

  it.each([
    ["missing backend opt-in", { writeSyncEnabled: undefined }],
    ["applied previews", { applied: true }],
    ["error previews", { error: "preview failed" }],
    ["missing revisions", { baselineRevision: "", remoteRevision: "" }],
  ])("blocks apply for %s", async (_label, overrides) => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_remote_playlist_sync_preview") return { ...preview("binding-1"), ...overrides };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(target.querySelector(".sync-apply")).toBeNull();
    expect(call).not.toHaveBeenCalledWith("apply_remote_playlist_sync_preview", expect.anything());
  });

  it("discards an in-flight preview when navigation changes the binding", async () => {
    let finishOldPreview!: (value: unknown) => void;
    const pendingOldPreview = new Promise(resolve => { finishOldPreview = resolve; });
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_remote_playlist_sync_preview") {
        if (args?.bindingId === "binding-1") return pendingOldPreview;
        return preview(String(args?.bindingId));
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const target = renderHost(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("get_remote_playlist_sync_preview", { bindingId: "binding-1" });
    target.querySelector<HTMLButtonElement>(".switch-binding")!.click(); await settle();
    finishOldPreview(preview("binding-1", "Old playlist preview")); await settle();
    expect(target.textContent).not.toContain("Old playlist preview");
    expect(target.querySelector(".sync-apply")).toBeNull();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("get_remote_playlist_sync_preview", { bindingId: "binding-2" });
    expect(target.textContent).toContain("spotify:track:binding-2");
  });

  it("ignores an apply result if navigation changes the binding while it is pending", async () => {
    let finishApply!: (value: unknown) => void;
    const pendingApply = new Promise(resolve => { finishApply = resolve; });
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_remote_playlist_sync_preview") return preview(String(args?.bindingId));
      if (command === "apply_remote_playlist_sync_preview") return pendingApply;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = renderHost(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".sync-apply")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("apply_remote_playlist_sync_preview", { previewId: "preview-binding-1" });
    target.querySelector<HTMLButtonElement>(".switch-binding")!.click(); await settle();
    finishApply({ ...preview("binding-1"), applied: true, summary: "Old playlist sync complete" }); await settle();
    expect(target.textContent).not.toContain("Old playlist sync complete");
    expect(target.querySelector(".sync-review")).toBeNull();
    expect(target.querySelector(".sync-apply")).toBeNull();
  });
});

describe("addition-only YouTube review", () => {
  it("shows the exact addition preview while leaving broad edit permission false", async () => {
    const value = { ...preview("binding-1"), provider: "youtube", canEdit: false, writeCapabilities: { add: true, remove: false, reorder: false } };
    const call = vi.fn(async (command: string) => {
      if (command === "get_remote_playlist_sync_preview") return value;
      if (command === "apply_remote_playlist_sync_preview") return { ...value, applied: true };
      throw Error(command);
    });
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(target.textContent).toContain("Additions only");
    expect(target.textContent).not.toContain("playlist is read-only");
    expect(target.querySelector(".sync-apply")).not.toBeNull();
    expect(call.mock.calls.some(([command]) => command === "apply_remote_playlist_sync_preview")).toBe(false);
    target.querySelector<HTMLButtonElement>(".sync-apply")!.click(); await settle();
    expect(call.mock.calls.filter(([command]) => command === "apply_remote_playlist_sync_preview")).toHaveLength(1);
  });
  it("blocks a mixed removal plan even if an explicit capability object accompanies legacy canEdit true", async () => {
    const call = vi.fn(async () => ({ ...preview("binding-1"), provider: "youtube", canEdit: true, writeCapabilities: { add: true, remove: false, reorder: false }, changes: { add: [], remove: [{ entryIds: ["occurrence-1"] }], move: [], unresolved: [] } }));
    const { target } = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".sync-trigger")!.click(); await settle();
    expect(target.querySelector(".sync-apply")).toBeNull();
    expect(call).toHaveBeenCalledTimes(1);
  });
});
