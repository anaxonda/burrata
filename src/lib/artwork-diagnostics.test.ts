// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { inspectArtworkImages } from "./artwork-diagnostics";

class FakeImage {
  static latest: FakeImage;
  src = "";
  currentSrc = "";
  naturalWidth = 0;
  naturalHeight = 0;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor() { FakeImage.latest = this; }
}
const privateAsset = "asset://localhost/%2Fprivate%2Fartwork%2Fcover.jpg";
afterEach(() => { document.body.innerHTML = ""; vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
describe("fixed player artwork diagnostics", () => {
  it("reports missing images and redacts remote artwork without requesting it", async () => {
    const construct = vi.fn(); vi.stubGlobal("Image", construct);
    const report = await inspectArtworkImages(document, "https://private.example/secret.jpg?token=secret", true);
    expect(report).toMatchObject({ urlScheme: "https", blacklisted: true, probeStatus: "not_local", probeOnError: false });
    expect(report.images).toEqual([
      { role: "footer", present: false, complete: false, naturalWidth: 0, naturalHeight: 0, urlScheme: "none", srcScheme: "none", currentSrcScheme: "none", srcMatchesExpected: false, currentSrcMatchesExpected: false, loadState: "absent" },
      { role: "details", present: false, complete: false, naturalWidth: 0, naturalHeight: 0, urlScheme: "none", srcScheme: "none", currentSrcScheme: "none", srcMatchesExpected: false, currentSrcMatchesExpected: false, loadState: "absent" },
      { role: "fullscreen", present: false, complete: false, naturalWidth: 0, naturalHeight: 0, urlScheme: "none", srcScheme: "none", currentSrcScheme: "none", srcMatchesExpected: false, currentSrcMatchesExpected: false, loadState: "absent" },
    ]);
    expect(construct).not.toHaveBeenCalled();
    expect(JSON.stringify(report)).not.toMatch(/private|secret|https:\/\//);
  });
  it("measures only actual footer and details images with URL schemes and expected-path matches", async () => {
    document.body.innerHTML = '<footer class="player"><div class="mini-art"><img></div></footer><aside class="playing-info"><div class="track-hero"><img></div></aside><img src="https://private.example/unrelated.jpg">';
    const image = document.querySelector<HTMLImageElement>("footer img")!;
    image.src = privateAsset; image.setAttribute("src", privateAsset);
    Object.defineProperties(image, { complete: { value: true }, naturalWidth: { value: 640 }, naturalHeight: { value: 640 }, currentSrc: { value: "http://asset.localhost/%2Fprivate%2Fartwork%2Fcover.jpg?x=1" } });
    const details = document.querySelector<HTMLImageElement>(".playing-info img")!;
    details.src = "https://private.example/footer.jpg";
    const report = await inspectArtworkImages(document, privateAsset, false);
    expect(report.images).toHaveLength(3);
    expect(report.images[0]).toMatchObject({ role: "footer", present: true, complete: true, naturalWidth: 640, naturalHeight: 640, urlScheme: "http", srcScheme: "asset", currentSrcScheme: "http", srcMatchesExpected: true, currentSrcMatchesExpected: true, loadState: "loaded" });
    expect(report.images[1]).toMatchObject({ role: "details", srcScheme: "https", srcMatchesExpected: false });
    expect(JSON.stringify(report)).not.toMatch(/private|cover\.jpg|footer\.jpg|asset:\/\/|asset\.localhost/);
  });
  it("preserves successful probe load status and dimensions with sanitized src identity", async () => {
    vi.stubGlobal("Image", FakeImage);
    const pending = inspectArtworkImages(document, privateAsset, true);
    const requested = new URL(FakeImage.latest.src);
    expect(`${requested.protocol}//${requested.host}${requested.pathname}`).toBe(privateAsset);
    expect(requested.searchParams.get("burrataArtworkProbe")).toMatch(/^\d+$/);
    FakeImage.latest.naturalWidth = 300; FakeImage.latest.naturalHeight = 300;
    FakeImage.latest.currentSrc = FakeImage.latest.src;
    FakeImage.latest.onload!();
    const report = await pending;
    expect(report).toMatchObject({ probeStatus: "loaded", probeWidth: 300, probeHeight: 300, cspBlocked: false, probeSrcScheme: "asset", probeCurrentSrcScheme: "asset", probeSrcMatchesExpected: true, probeCurrentSrcMatchesExpected: true, probeOnError: false });
    expect(FakeImage.latest.onload).toBeNull(); expect(FakeImage.latest.onerror).toBeNull();
    expect(JSON.stringify(report)).not.toMatch(/private|cover\.jpg|asset:\/\/|asset\.localhost/);
  });
  it("reports probe onerror and a cross-form currentSrc match without returning URLs", async () => {
    vi.stubGlobal("Image", FakeImage);
    const pending = inspectArtworkImages(document, privateAsset, true);
    FakeImage.latest.currentSrc = "http://asset.localhost/%2Fprivate%2Fartwork%2Fcover.jpg?burrataArtworkProbe=1";
    FakeImage.latest.onerror!();
    const report = await pending;
    expect(report).toMatchObject({ probeStatus: "failed", probeWidth: 0, probeHeight: 0, cspBlocked: false, probeSrcScheme: "asset", probeCurrentSrcScheme: "http", probeSrcMatchesExpected: true, probeCurrentSrcMatchesExpected: true, probeOnError: true });
    expect(FakeImage.latest.onload).toBeNull(); expect(FakeImage.latest.onerror).toBeNull();
    expect(JSON.stringify(report)).not.toMatch(/private|cover\.jpg|asset:\/\/|asset\.localhost/);
  });
  it("rejects unsupported authorities, ports, NUL and malformed escapes without requesting", async () => {
    const construct = vi.fn(); vi.stubGlobal("Image", construct);
    for (const url of [
      "asset://user@localhost/%2Fprivate%2Fartwork%2Fcover.jpg",
      "asset://@localhost/%2Fprivate%2Fartwork%2Fcover.jpg",
      "asset://:secret@localhost/%2Fprivate%2Fartwork%2Fcover.jpg",
      "asset://localhost:8080/%2Fprivate%2Fartwork%2Fcover.jpg",
      "http://asset.localhost:80/%2Fprivate%2Fartwork%2Fcover.jpg",
      "asset://other.localhost/%2Fprivate%2Fartwork%2Fcover.jpg",
      "asset://localhost/%2Fprivate%2Fartwork%2F%00cover.jpg",
      "asset://localhost/%2Fprivate%2Fartwork%2F%GGcover.jpg",
    ]) {
      const report = await inspectArtworkImages(document, url, false);
      expect(report.probeStatus).toBe("not_local");
      expect(report.probeSrcScheme).toBe("none");
    }
    expect(construct).not.toHaveBeenCalled();
  });
  it("bounds local probe time and removes the policy listener", async () => {
    vi.useFakeTimers(); vi.stubGlobal("Image", FakeImage);
    const remove = vi.spyOn(document, "removeEventListener");
    const pending = inspectArtworkImages(document, privateAsset, false);
    await vi.advanceTimersByTimeAsync(1500);
    expect(await pending).toMatchObject({ probeStatus: "timeout", probeWidth: 0, probeOnError: false });
    expect(remove).toHaveBeenCalledWith("securitypolicyviolation", expect.any(Function));
    expect(FakeImage.latest.onerror).toBeNull();
  });
  it("reports a matching image CSP failure without returning policy or blocked URLs", async () => {
    vi.stubGlobal("Image", FakeImage);
    const pending = inspectArtworkImages(document, privateAsset, false);
    const irrelevant = new Event("securitypolicyviolation");
    Object.assign(irrelevant, { effectiveDirective: "connect-src", blockedURI: privateAsset });
    document.dispatchEvent(irrelevant);
    const blocked = new Event("securitypolicyviolation");
    Object.assign(blocked, { effectiveDirective: "img-src", blockedURI: "asset://localhost" });
    document.dispatchEvent(blocked);
    FakeImage.latest.onerror!();
    const report = await pending;
    expect(report).toMatchObject({ probeStatus: "failed", cspBlocked: true, probeOnError: true });
    expect(JSON.stringify(report)).not.toContain("private");
  });
});

it("measures fullscreen image dimensions without returning its remote URL",async()=>{
  document.body.innerHTML='<section class="now-playing-view"><div class="hero-art"><img src="https://private.invalid/image?secret=1"></div></section>';
  const image=document.querySelector("img")!;
  Object.defineProperties(image,{complete:{value:true},naturalWidth:{value:1200},naturalHeight:{value:1200}});
  const report=await inspectArtworkImages(document,"",false);
  expect(report.images[2]).toMatchObject({role:"fullscreen",present:true,complete:true,naturalWidth:1200,naturalHeight:1200,loadState:"loaded"});
  expect(JSON.stringify(report)).not.toMatch(/private|secret|https:\/\//);
});
