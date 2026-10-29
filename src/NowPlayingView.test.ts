// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Svelte browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import NowPlayingView from "./NowPlayingView.svelte";
import type { Track } from "./lib/types";
import type { ArtistProfile } from "./lib/artist-types";
let cleanup: (() => Promise<void>) | undefined;
afterEach(async () => { await cleanup?.(); document.body.innerHTML = ""; });
const track: Track = {id:"one", provider:"spotify", providerId:"one", sourceUrl:"",title:"Turtledove",artist:"Domna Samiou, Michalis Zambas",album:"The Great North Wind",favorite:false,provenance:{}};
function setup(overrides: Record<string, unknown> = {}) {
 const props = {track,title:track.title,loadArtist:vi.fn().mockResolvedValue(null),onClose:vi.fn(),onArtist:vi.fn(),onQueue:vi.fn(),onFavorite:vi.fn(),...overrides};
 const instance = mount(NowPlayingView,{target:document.body,props}); cleanup=()=>unmount(instance); return props;
}
it("keeps missing metadata usable and routes queue, favorites and Escape to the existing player",async()=>{
 const props=setup(); await tick(); await tick();
 expect(document.querySelector("h1")?.textContent).toBe("Turtledove");
 expect(document.body.textContent).toContain("No more tracks queued");
 await vi.waitFor(() => expect(document.body.textContent).toContain("Artist information is not available"));
 expect(document.querySelector(".hero-art .placeholder")).not.toBeNull();
 (document.querySelector(".favorite") as HTMLButtonElement).click();
 (Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Open queue")!).click();
 window.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape"}));
 expect(props.onFavorite).toHaveBeenCalledOnce(); expect(props.onQueue).toHaveBeenCalledOnce(); expect(props.onClose).toHaveBeenCalledOnce();
 expect(document.body.textContent).not.toContain("Main Artist");
});
it("shows available biography and next queue occurrence without starting playback",async()=>{
 const profile={artist:{name:"Domna Samiou"},biography:"A performer and researcher.",biographySource:"MusicBrainz"} as ArtistProfile;
 const loader=vi.fn().mockResolvedValue(profile);
 setup({loadArtist:loader,nextTrack:{...track,id:"two",title:"Ederlezi"}}); await tick();await tick();
 expect(loader).toHaveBeenCalledWith(track);
 expect(document.body.textContent).toContain("A performer and researcher.");
 expect(document.querySelector(".next-track")?.textContent).toContain("Ederlezi");
});
it("restores keyboard focus to the opener on close",async()=>{
 const opener=document.createElement("button");document.body.append(opener);opener.focus();
 setup();await tick(); expect(document.activeElement?.getAttribute("aria-label")).toBe("Close full-screen Now Playing");
 await cleanup?.();cleanup=undefined;expect(document.activeElement).toBe(opener);
});
it("offers set, detected, and side-by-side fullscreen art choices and falls back when detection art is missing",async()=>{
 const onArtworkModeChange=vi.fn();
 setup({detectedArtworkUrl:"https://example.test/detected.jpg",artworkMode:"both",onArtworkModeChange});await tick();
 const artworkMenu=document.querySelector<HTMLDetailsElement>('.artwork-menu')!;
 expect(artworkMenu.open).toBe(false);
 expect(document.querySelector('.artwork-choice')?.closest('header')).not.toBeNull();
 expect(document.querySelector('.hero-art')?.classList.contains('both-art')).toBe(true);
 expect(document.querySelector('.hero-art img[alt="Detected track artwork"]')?.getAttribute('src')).toBe("https://example.test/detected.jpg");
 (document.querySelector('.artwork-menu summary') as HTMLElement).click();
 expect(artworkMenu.open).toBe(true);
 expect(artworkMenu.querySelectorAll('.artwork-choice input')).toHaveLength(3);
 (artworkMenu.querySelectorAll('.artwork-choice input')[1] as HTMLInputElement).click();
 expect(onArtworkModeChange).toHaveBeenCalledWith("detected");
 expect(artworkMenu.open).toBe(false);
 await cleanup?.();cleanup=undefined;document.body.innerHTML="";
 setup({artworkMode:"detected",detectedArtworkUrl:null});await tick();
 expect(document.querySelector('.hero-art .placeholder')).not.toBeNull();
});
it("forwards lyrics seeking and the player bottom inset",async()=>{
 const onSeek=vi.fn();
 const loadLyrics=vi.fn().mockResolvedValue({status:"synced",provider:"lrclib",lines:[{id:"line-1",timeSeconds:12,text:"A timed lyric"}]});
 setup({onSeek,bottom:123,loadLyrics});await tick();
 expect(document.querySelector<HTMLElement>('.now-playing-view')?.style.bottom).toBe("123px");
 (document.querySelector('.lyrics-open') as HTMLButtonElement).click();
 await vi.waitFor(()=>expect(document.querySelector('.lyrics-seek-line')).not.toBeNull());
 (document.querySelector('.lyrics-seek-line') as HTMLButtonElement).click();
 expect(onSeek).toHaveBeenCalledWith(12);
 (document.querySelector('[aria-label="Maximize lyrics"]') as HTMLButtonElement).click();await tick();
 expect(document.querySelector<HTMLElement>('[aria-label="Fullscreen lyrics"]')?.style.bottom).toBe("123px");
});

it("dismisses the artwork menu before Escape exits Now Playing",async()=>{
 const props=setup({detectedArtworkUrl:"https://example.test/detected.jpg",onArtworkModeChange:vi.fn()});await tick();
 const menu=document.querySelector<HTMLDetailsElement>('.artwork-menu')!;
 const trigger=menu.querySelector('summary')!;
 trigger.click();
 trigger.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true}));
 expect(menu.open).toBe(false);
 expect(props.onClose).not.toHaveBeenCalled();
 expect(document.activeElement).toBe(trigger);
 trigger.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true}));
 expect(props.onClose).toHaveBeenCalledOnce();
});
