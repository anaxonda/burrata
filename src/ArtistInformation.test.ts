// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { expect, it, vi } from "vitest";
const { callMock } = vi.hoisted(() => ({ callMock: vi.fn().mockResolvedValue(undefined) }));
vi.mock("./lib/api", () => ({ call: callMock, desktop: () => true }));
// @ts-expect-error Svelte browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ArtistInformation from "./ArtistInformation.svelte";
// @ts-expect-error Exercise parent prop updates with Svelte's reactive proxy.
import { proxy } from "../node_modules/svelte/src/internal/client/proxy.js";

it("retries song context for the same station and artist and ignores an outdated answer", async () => {
  const target = document.createElement("div"); document.body.append(target);
  let finishFirst!: (value: unknown) => void;
  const loadArtist = vi.fn().mockImplementationOnce(() => new Promise(resolve => {finishFirst=resolve;}))
    .mockResolvedValueOnce({artist:{name:"Beck"},biography:"Current song artist biography.",links:[]});
  const props = proxy({track:{id:"radio:mellow",artist:"Beck",title:"Earlier song",isrc:null},loadArtist,onArtist:()=>{}});
  const instance = mount(ArtistInformation,{target,props});
  try {
    await tick();
    props.track={...props.track,title:"Morning"};
    await tick(); await tick();
    expect(loadArtist).toHaveBeenCalledTimes(2);
    expect(target.textContent).toContain("Current song artist biography.");
    finishFirst({artist:{name:"Other Beck"},biography:"Outdated biography.",links:[]});
    await tick(); await tick();
    expect(target.textContent).not.toContain("Outdated biography.");
    expect(target.textContent).toContain("Current song artist biography.");
  } finally { await unmount(instance); target.remove(); }
});

it("attributes a Wikipedia biography and opens its article and license", async () => {
  const target = document.createElement("div"); document.body.append(target);
  const url = "https://en.wikipedia.org/wiki/Example_artist";
  const profile = { artist: {name:"Example"}, biography:"An artist.", biographySource:"Wikipedia", links:[{provider:"wikipedia",label:"Wikipedia biography",url}] };
  const instance = mount(ArtistInformation, {target, props:{track:{id:"1",artist:"Example"},loadArtist:async()=>profile,onArtist:()=>{}}});
  await tick(); await tick();
  const buttons = [...target.querySelectorAll("button")];
  buttons.find(button => button.textContent?.includes("Wikipedia"))!.click();
  buttons.find(button => button.textContent?.includes("CC BY-SA"))!.click();
  expect(callMock).toHaveBeenCalledWith("open_external_url", {url});
  expect(callMock).toHaveBeenCalledWith("open_external_url", {url:"https://creativecommons.org/licenses/by-sa/4.0/"});
  await unmount(instance); target.remove();
});

it("renders Discogs biography links safely and preserves paragraph breaks", async () => {
  const target = document.createElement("div"); document.body.append(target);
  const profile = { artist: {name:"Marty Robbins"}, biography:"First\r\n\r\n[url=https://en.wikipedia.org/wiki/Little_Jimmy_Dickens]Jimmy Dickens[/url] [a2118004] <script>bad</script>", biographySource:"Discogs", links:[] };
  const instance = mount(ArtistInformation, {target, props:{track:{id:"2",artist:"Marty Robbins"},loadArtist:async()=>profile,onArtist:()=>{}}});
  await tick(); await tick();
  const paragraph = target.querySelector("p")!;
  expect(paragraph.textContent).toContain("First\n\nJimmy Dickens Artist on Discogs <script>bad</script>");
  expect(paragraph.querySelector("script")).toBeNull();
  [...paragraph.querySelectorAll("button")].find(button => button.textContent === "Jimmy Dickens")!.click();
  expect(callMock).toHaveBeenCalledWith("open_external_url", {url:"https://en.wikipedia.org/wiki/Little_Jimmy_Dickens"});
  await unmount(instance); target.remove();
});
