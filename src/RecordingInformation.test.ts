// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Svelte browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RecordingInformation from "./RecordingInformation.svelte";
import RecordingInformationHarness from "./test-harnesses/RecordingInformationHarness.svelte";
import type { Track } from "./lib/types";
import type { RecordingCatalogInfo } from "./lib/recording-information";
const apiCall = vi.hoisted(() => vi.fn());
vi.mock("./lib/api", () => ({call: apiCall}));
const track: Track = {id:"radio-song",provider:"radio_song",providerId:"one",sourceUrl:"",title:"Love Like This",artist:"Faith Evans",favorite:false,provenance:{}};
const recording: RecordingCatalogInfo = {provider:"musicbrainz",id:"recording",title:track.title,artist:track.artist,version:null,firstReleaseDate:"1998",durationSeconds:240,isrcs:[],genres:["R&B"],sourceUrl:"",recordingArtist:{name:track.artist,artist:{provider:"musicbrainz",id:"artist"}},releases:[{provider:"musicbrainz",id:"album",title:"Keep the Faith",date:"1998",country:"US",sourceUrl:""}]};
let instance: ReturnType<typeof mount> | undefined;
afterEach(async()=>{if(instance){await unmount(instance);instance=undefined;}document.body.innerHTML="";});
it("routes discovered artist and album links without changing the saved track",async()=>{
 const onArtist=vi.fn(),onRelease=vi.fn(); const before=JSON.stringify(track);
 instance=mount(RecordingInformation,{target:document.body,props:{track,loadRecording:async()=>({resolution:"exact_isrc",recording,candidates:[]}),onArtist,onRelease}});
 await tick();await tick();
 (document.querySelector(".artist") as HTMLButtonElement).click();
 (document.querySelector(".release") as HTMLButtonElement).click();
 expect(onArtist).toHaveBeenCalledWith(recording.recordingArtist!.artist);
 expect(onRelease).toHaveBeenCalledWith(recording.releases[0],recording.recordingArtist!.artist);
 expect(JSON.stringify(track)).toBe(before);
 expect(document.body.textContent).toContain("R&B");
});
it("requires choosing an uncertain catalog suggestion before exposing its album",async()=>{
 instance=mount(RecordingInformation,{target:document.body,props:{track,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[{recording,identity:"text_suggestion"}]}),onRelease:vi.fn()}});
 await tick();await tick();
 expect(document.querySelector(".release")).toBeNull();
 (document.querySelector(".candidate") as HTMLButtonElement).click();await tick();
 expect(document.body.textContent).toContain("Catalog suggestion");
 expect(document.querySelector(".release")?.textContent).toContain("Keep the Faith");
});
it("shows imported page links and public credits without treating them as catalog proof",async()=>{
 const imported:Track={...track,provider:"bandcamp",sourceUrl:"https://artist.bandcamp.com/track/song",provenance:{playbackMode:"lookup",importMetadata:{albumUrl:"https://artist.bandcamp.com/album/release",artistUrl:"https://artist.bandcamp.com/",credits:"Recorded by Artist",tags:["alternative"]}}};
 instance=mount(RecordingInformation,{target:document.body,props:{track:imported,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 expect(document.querySelector('a[href="https://artist.bandcamp.com/album/release"]')?.textContent).toContain("Album page");
 expect(document.querySelector('a[href="https://artist.bandcamp.com/track/song"]')).toBeNull();
 expect(document.body.textContent).toContain("Recorded by Artist");
 expect(document.body.textContent).toContain("Source tags: alternative");
 expect((document.querySelector('[aria-label="Playback source preference"]') as HTMLSelectElement).value).toBe("lookup");
 expect(document.querySelector('.release')).toBeNull();
});

it("keeps exact-upload playback preferences separate from absent imported metadata",async()=>{
 const upload={...track,provider:"youtube",sourceUrl:"https://www.youtube.com/watch?v=example",provenance:{}};
 instance=mount(RecordingInformation,{target:document.body,props:{track:upload,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 expect(document.body.textContent).not.toContain("Imported source information");
 const select=document.querySelector<HTMLSelectElement>('[aria-label="Playback source preference"]')!;
 expect(select.closest('details')).toBeNull();
 expect(select.value).toBe("source");
 expect(select.textContent).toContain("Keep this exact upload");
 expect(select.textContent).toContain("Allow matching sources");
 expect(document.body.textContent).toContain("Applies next time you play");
});

it("does not call the configured AI lookup on mount and hides it without configuration",async()=>{
 apiCall.mockReset();
 instance=mount(RecordingInformation,{target:document.body,props:{track,contextLookupConfigured:false,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 expect(document.querySelector(".context-action")).toBeNull();
 expect(document.querySelector('[aria-label="AI song and set context"]')).toBeNull();
 expect(apiCall).not.toHaveBeenCalled();
});

it("sends only the allowlisted metadata on an explicit click and renders the answer as plain escaped text",async()=>{
 apiCall.mockReset().mockImplementation(async(_command:string,args:{request:{requestId:string}})=>({requestId:args.request.requestId,identity:"opaque",answer:"<img src=x onerror=alert(1)> https://untrusted.example/claim",model:"model",provider:"openaiCompatible"}));
 const source={...track,provider:"youtube",album:"Live at Somewhere",version:"Live",recordingDate:"1998-04-03",sourceUrl:"https://www.youtube.com/watch?v=private",provenance:{importMetadata:{description:"must not be sent"}}};
 instance=mount(RecordingInformation,{target:document.body,props:{track:source,contextLookupConfigured:true,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 expect(apiCall).not.toHaveBeenCalled();
 (document.querySelector(".context-action") as HTMLButtonElement).click();
 await tick();await tick();
 expect(apiCall).toHaveBeenCalledTimes(1);
 const [command,args]=apiCall.mock.calls[0];
 expect(command).toBe("lookup_media_context");
 expect(args.request).toMatchObject({title:track.title,artist:track.artist,album:"Live at Somewhere",version:"Live",sourceKind:"youtube",targetKind:"song",date:"1998-04-03"});
 expect(args.request).not.toHaveProperty("sourceUrl");
 expect(args.request).not.toHaveProperty("description");
 expect(args.request.requestId).toMatch(/^[0-9a-f-]{36}$/i);
 expect(document.querySelector(".context-result")?.textContent).toContain("AI-generated · unverified");
 expect(document.querySelector(".context-result img")).toBeNull();
 expect(document.querySelector(".context-result a")).toBeNull();
 expect(document.querySelector(".context-result")?.textContent).toContain("<img src=x onerror=alert(1)>");
});

it("lets the user choose song, concert, or set context explicitly",async()=>{
 apiCall.mockReset().mockImplementation(async(_command:string,args:{request:Record<string,unknown>})=>({requestId:args.request.requestId,identity:"opaque",answer:"Context",model:"model",provider:"openaiCompatible"}));
 instance=mount(RecordingInformation,{target:document.body,props:{track,contextLookupConfigured:true,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 const kind=document.querySelector<HTMLSelectElement>('[aria-label="Context type"]')!;
 kind.value="set";kind.dispatchEvent(new Event("change"));await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();
 await tick();await tick();
 expect(apiCall.mock.calls[0][1].request.targetKind).toBe("set");
 expect(document.querySelector(".context-result strong")?.textContent).toContain("This DJ set");
});

it("separates answer labeling from the answer and renders only valid returned sources",async()=>{
 apiCall.mockReset().mockImplementation(async(_command:string,args:{request:{requestId:string}})=>({
  requestId:args.request.requestId,identity:"opaque",answer:"The event took place at Alexandra Palace.",model:"model",provider:"openaiCompatible",
  sources:[
   {title:"Official event page",url:"https://venue.example/events/usb002"},
   {title:"Local address",url:"https://127.0.0.1/private"},
   {title:"Unencrypted page",url:"http://news.example/story"},
  ],
 }));
 instance=mount(RecordingInformation,{target:document.body,props:{track,contextLookupConfigured:true,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();
 await tick();await tick();
 const result=document.querySelector(".context-result")!;
 expect(result.querySelector(".context-badge")?.textContent).toContain("web sources");
 expect(result.querySelector(".context-result>strong")?.textContent).toContain(track.title);
 expect(result.querySelector(".context-result>p")?.textContent).toBe("The event took place at Alexandra Palace.");
 expect(result.querySelectorAll(".context-sources a")).toHaveLength(1);
 expect(result.querySelector(".context-sources a")?.textContent).toContain("Official event page");
});

it("sends a canonical public YouTube locator and omits private or nonstandard source URLs",async()=>{
 apiCall.mockReset().mockImplementation(async(_command:string,args:{request:{requestId:string}})=>({requestId:args.request.requestId,identity:"opaque",answer:"Context",model:"model",provider:"openaiCompatible"}));
 const youtube={...track,provider:"youtube",sourceUrl:"https://www.youtube.com/watch?v=AbCdEf123_-&list=private&si=secret#t=2"};
 instance=mount(RecordingInformation,{target:document.body,props:{track:youtube,contextLookupConfigured:true,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();await tick();await tick();
 expect(apiCall.mock.calls[0][1].request.sourceUrl).toBe("https://www.youtube.com/watch?v=AbCdEf123_-");
 await unmount(instance);instance=undefined;document.body.innerHTML="";

 const privateSoundcloud={...track,provider:"soundcloud",sourceUrl:"https://soundcloud.com/artist/track/s-private-token"};
 instance=mount(RecordingInformation,{target:document.body,props:{track:privateSoundcloud,contextLookupConfigured:true,loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();await tick();await tick();
 expect(apiCall.mock.calls[1][1].request).not.toHaveProperty("sourceUrl");
});

it("offers separate detected song and full-set lookups in fullscreen context",async()=>{
 apiCall.mockReset().mockImplementation(async(_command:string,args:{request:Record<string,unknown>})=>({requestId:args.request.requestId,identity:"opaque",answer:"Context",model:"model",provider:"openaiCompatible"}));
 const detected={...track,title:"Detected song",artist:"Detected artist"};
 const fullSet={...track,provider:"youtube",title:"Long DJ mix",artist:"DJ",sourceUrl:"https://youtube.com/watch?v=private"};
 instance=mount(RecordingInformation,{target:document.body,props:{track:fullSet,contextLookupConfigured:true,contextTargets:[{track:detected,label:"Detected song",targetKind:"song"},{track:fullSet,label:"Full set",targetKind:"set"}],loadRecording:async()=>({resolution:"unresolved",recording:null,candidates:[]})}});
 await tick();await tick();
 const buttons=[...document.querySelectorAll<HTMLButtonElement>(".context-action")];
 expect(buttons.map(button=>button.textContent)).toEqual(["Ask AI about Detected song","Ask AI about Full set"]);
 buttons[0].click();await tick();await tick();
 expect(apiCall.mock.calls[0][1].request).toMatchObject({title:"Detected song",artist:"Detected artist",targetKind:"song"});
});

it("suppresses stale lookup success and error across A to B to A metadata changes",async()=>{
 apiCall.mockReset();
 const pending: Array<{resolve:(value:unknown)=>void;reject:(reason:unknown)=>void}> = [];
 apiCall.mockImplementation(()=>new Promise((resolve,reject)=>pending.push({resolve,reject})));
 instance=mount(RecordingInformationHarness,{target:document.body});
 await tick();await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();
 await tick();
 window.dispatchEvent(new CustomEvent("recording-information-test-track",{detail:{...track,title:"B"}}));await tick();await tick();
 window.dispatchEvent(new CustomEvent("recording-information-test-track",{detail:{...track,title:"A"}}));await tick();await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();
 await tick();
 expect(apiCall).toHaveBeenCalledTimes(2);
 pending[0].resolve({requestId:apiCall.mock.calls[0][1].request.requestId,identity:"one",answer:"stale A success",model:"m",provider:"openaiCompatible"});
 await tick();
 expect(document.body.textContent).not.toContain("stale A success");
 pending[1].resolve({requestId:apiCall.mock.calls[1][1].request.requestId,identity:"two",answer:"fresh A success",model:"m",provider:"openaiCompatible"});
 await tick();await tick();
 expect(document.body.textContent).toContain("fresh A success");

 apiCall.mockReset();
 pending.length=0;
 apiCall.mockImplementation(()=>new Promise((resolve,reject)=>pending.push({resolve,reject})));
 (document.querySelector(".context-action") as HTMLButtonElement).click();
 await tick();
 window.dispatchEvent(new CustomEvent("recording-information-test-track",{detail:{...track,title:"B"}}));await tick();await tick();
 window.dispatchEvent(new CustomEvent("recording-information-test-track",{detail:{...track,title:"A"}}));await tick();await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();
 await tick();
 expect(apiCall).toHaveBeenCalledTimes(2);
 pending[0].reject(new Error("stale failure"));
 await tick();
 expect(document.querySelector('[role="alert"]')).toBeNull();
 pending[1].reject(new Error("current failure"));
 await tick();await tick();
 expect(document.querySelector('[role="alert"]')?.textContent).toContain("Could not look up context");
});

it("discards a pending answer after the configured AI connection changes",async()=>{
 apiCall.mockReset();
 let resolveLookup:(value:unknown)=>void=()=>{};
 apiCall.mockImplementation(()=>new Promise(resolve=>{resolveLookup=resolve;}));
 instance=mount(RecordingInformationHarness,{target:document.body});
 await tick();await tick();
 (document.querySelector(".context-action") as HTMLButtonElement).click();await tick();
 window.dispatchEvent(new Event("recording-information-test-config"));await tick();await tick();
 resolveLookup({requestId:apiCall.mock.calls[0][1].request.requestId,identity:"opaque",answer:"answer from old connection",model:"m",provider:"openaiCompatible"});
 await tick();await tick();
 expect(document.body.textContent).not.toContain("answer from old connection");
 expect(document.querySelector(".context-action")?.textContent).toContain("Ask AI");
});
