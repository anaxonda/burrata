// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Svelte browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RecordingInformation from "./RecordingInformation.svelte";
import type { Track } from "./lib/types";
import type { RecordingCatalogInfo } from "./lib/recording-information";
const track: Track = {id:"radio-song",provider:"radio_song",providerId:"one",sourceUrl:"",title:"Love Like This",artist:"Faith Evans",favorite:false,provenance:{}};
const recording: RecordingCatalogInfo = {provider:"musicbrainz",id:"recording",title:track.title,artist:track.artist,version:null,firstReleaseDate:"1998",durationSeconds:240,isrcs:[],genres:["R&B"],sourceUrl:"",recordingArtist:{name:track.artist,artist:{provider:"musicbrainz",id:"artist"}},releases:[{provider:"musicbrainz",id:"album",title:"Keep the Faith",date:"1998",country:"US",sourceUrl:""}]};
let instance: ReturnType<typeof mount>;
afterEach(async()=>{if(instance) await unmount(instance);document.body.innerHTML="";});
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
 expect(document.body.textContent).toContain("Recorded by Artist");
 expect(document.body.textContent).toContain("Source tags: alternative");
 expect((document.querySelector('[aria-label="Playback source preference"]') as HTMLSelectElement).value).toBe("lookup");
 expect(document.querySelector('.release')).toBeNull();
});
