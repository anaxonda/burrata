// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime lacks public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import AutotagReview from "./AutotagReview.svelte";
import type { LocalAsset } from "./lib/types";

let destroy: (() => void) | undefined;
afterEach(() => {
  destroy?.();
  destroy = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const asset = {
  id: "asset-1",
  path: "/tmp/por-ella.flac",
  hash: "hash-1",
  track: { id: "track-1", provider: "youtube", providerId: "video-1", sourceUrl: "", artist: "YouTube Guy Bertrand", title: "Por ella", favorite: false, provenance: {} },
} as LocalAsset;

async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }

function render(
  job: Record<string, unknown>,
  onApplyRecognition?: (assetId: string, fieldKeys: ("artist" | "title" | "isrc")[]) => unknown,
  reviewAsset: LocalAsset = asset,
  onRepairSource?: (assetId: string) => unknown,
  onRetry = vi.fn(),
  onApply = vi.fn(),
) {
  const target = document.createElement("div");
  document.body.append(target);
  const instance = mount(AutotagReview, {
    target,
    props: {
      job: { assetId: asset.id, status: "review", candidates: [], ...job },
      asset: reviewAsset,
      onReview: vi.fn(),
      onApply,
      onApplyRecognition,
      onRepairSource,
      onRetry,
    } as never,
  });
  destroy = () => unmount(instance);
  return target;
}

describe("AutotagReview recognition evidence", () => {
  it.each([
    {agreement: {isrc:"FRZ089900030"}, expected:true},
    {agreement: {isrc:"FRZ089900031"}, expected:false},
    {agreement: undefined, expected:false},
  ])("labels double-excerpt ISRC evidence only when explicitly retained: $expected", ({agreement, expected}) => {
    const target=render({recognition:{provider:"songrec",artist:"Synthetic artist",title:"Synthetic title",
      isrc:"FRZ089900030",excerptCount:2,agreeingIdentifiers:agreement}});
    expect(target.querySelector(".recognition-isrc")?.textContent?.includes("agrees in both excerpts")).toBe(expected);
  });

  it("shows two-excerpt recognition and ISRC while marking catalog identity unverified", () => {
    const target = render({
      message: "Audio recognition matched two separate excerpts, but no unique catalog recording was resolved.",
      recognition: {
        provider: "songrec",
        artist: "Tékaméli",
        title: "Por Ella",
        isrc: "FRZ089900030",
        shazamId: "10222041",
        excerptCount: 2,
      },
    });
    const panel = target.querySelector('[aria-label="Audio recognition evidence"]');
    expect(panel?.textContent).toContain("catalog identity is unverified");
    expect(panel?.textContent).toContain("Tékaméli");
    expect(panel?.textContent).toContain("Por Ella");
    expect(panel?.textContent).toContain("FRZ089900030");
    expect(target.querySelector(".review-actions")).toBeNull();
  });

  it("keeps legacy jobs without recognition evidence renderable", () => {
    const target = render({});
    expect(target.querySelector('[aria-label="Audio recognition evidence"]')).toBeNull();
    expect(target.textContent).toContain("No match is available yet.");
  });

  it("lets a user select and apply only the recognized fields they approve", async () => {
    const onApplyRecognition = vi.fn();
    const target = render({
      recognition: {
        provider: "songrec", artist: "Tékaméli", title: "Por Ella", isrc: "FRZ089900030", excerptCount: 2,
      },
    }, onApplyRecognition);
    const panel = target.querySelector('[aria-label="Review recognized metadata"]');
    expect(panel?.textContent).toContain("Apply recognized tags");
    const boxes = [...panel!.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    expect(boxes).toHaveLength(3);
    boxes[0]!.click();
    boxes[2]!.click();
    await settle();
    panel!.querySelector<HTMLButtonElement>("button")!.click();
    await settle();
    expect(onApplyRecognition).toHaveBeenCalledWith("asset-1", ["artist", "isrc"]);
  });

  it("does not offer already-applied recognition fields again", () => {
    const target = render({
      status: "recognition_applied",
      recognition: { provider: "songrec", artist: "Tékaméli", title: "Por Ella", excerptCount: 2 },
    }, vi.fn());
    expect(target.querySelector('[aria-label="Review recognized metadata"]')).toBeNull();
    expect(target.querySelector(".recognition-applied")?.textContent).toContain("Catalog identity remains unverified");
  });

  it("does not offer manually protected fields for recognition replacement", () => {
    const onApplyRecognition = vi.fn();
    const protectedAsset = {
      ...asset,
      track: { ...asset.track, provenance: { manualFields: { artist: true } } },
    } as LocalAsset;
    const target = render({
      recognition: { provider: "songrec", artist: "Tékaméli", title: "Por Ella", isrc: "FRZ089900030", excerptCount: 2 },
    }, onApplyRecognition, protectedAsset);
    const panel = target.querySelector('[aria-label="Review recognized metadata"]');
    expect(panel?.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')).toHaveLength(2);
    expect([...panel!.querySelectorAll(".field-name")].map((node) => node.textContent)).toEqual(["Title", "ISRC"]);
  });

  it("recovers local source tags through its own action without invoking retry or metadata apply", async () => {
    const onRepairSource = vi.fn().mockResolvedValue({
      assetId: asset.id, changed: true, fields: ["artist", "album"], identityStatus: "source_reported_unverified",
    });
    const onRetry = vi.fn();
    const onApply = vi.fn();
    const onApplyRecognition = vi.fn();
    const managedDownload = {
      ...asset,
      processing: "original audio codec preserved",
      track: { ...asset.track, artist: "", album: null, provenance: { publicationJobId: "job-1" } },
    } as LocalAsset;
    const target = render({ status: "review" }, onApplyRecognition, managedDownload, onRepairSource, onRetry, onApply);
    const button = [...target.querySelectorAll<HTMLButtonElement>("button")]
      .find((item) => item.textContent?.includes("Recover source tags"));
    expect(button).toBeTruthy();
    button!.click();
    await settle();
    expect(onRepairSource).toHaveBeenCalledTimes(1);
    expect(onRepairSource).toHaveBeenCalledWith("asset-1");
    expect(onRetry).not.toHaveBeenCalled();
    expect(onApply).not.toHaveBeenCalled();
    expect(onApplyRecognition).not.toHaveBeenCalled();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Catalog identity remains unverified");
  });

  it("hides source recovery for read-only or fully manually protected missing tags", () => {
    const assetWithMarker = {
      ...asset,
      processing: "original audio codec preserved",
      track: { ...asset.track, artist: "", album: null, provenance: { publicationJobId: "job-1", manualFields: { artist: true, album: true } } },
    } as LocalAsset;
    expect(render({}, undefined, assetWithMarker, vi.fn()).querySelector('[aria-label="Recover source tags"]')).toBeNull();
    const readOnly = { ...assetWithMarker, processing: "external-readonly", track: { ...assetWithMarker.track, provenance: { publicationJobId: "job-1" } } } as LocalAsset;
    expect(render({}, undefined, readOnly, vi.fn()).querySelector('[aria-label="Recover source tags"]')).toBeNull();
  });

  it("shows a local error when source recovery fails", async () => {
    const onRepairSource = vi.fn().mockRejectedValue(new Error("Source metadata is unavailable"));
    const managedDownload = {
      ...asset,
      processing: "original audio codec preserved",
      track: { ...asset.track, artist: "", album: null, provenance: { publicationJobId: "job-1" } },
    } as LocalAsset;
    const target = render({}, undefined, managedDownload, onRepairSource);
    target.querySelector<HTMLButtonElement>(".source-repair-button")!.click();
    await settle();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain("Source metadata is unavailable");
  });
});
