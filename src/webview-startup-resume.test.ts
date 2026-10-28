import { expect, it } from "vitest";
import { hasWebviewStartupResumeProgress, startupResumeBlocksHistory } from "./webview-startup-resume";

const resumed = {
  target: 349.437,
  startPosition: 349.437,
  currentTime: 349.8,
  duration: 7201.088,
  elapsedMs: 500,
  readyState: 4,
  seeking: false,
  paused: false,
  playbackRate: 1,
};

it("accepts advancing playback only after the saved resume target landed", () => {
  expect(hasWebviewStartupResumeProgress(resumed)).toBe(true);
  expect(hasWebviewStartupResumeProgress({ ...resumed, startPosition: 1.4, currentTime: 349.437 })).toBe(false);
  expect(hasWebviewStartupResumeProgress({ ...resumed, startPosition: 1.4, currentTime: 349.8 })).toBe(true);
  expect(hasWebviewStartupResumeProgress({ ...resumed, readyState: 0, currentTime: 349.8 })).toBe(false);
  expect(hasWebviewStartupResumeProgress({ ...resumed, seeking: true })).toBe(false);
});

it("rejects an optimistic 349 second assignment when metadata resets the media to 1.4 seconds", () => {
  const optimisticAssignment = { ...resumed, currentTime: 349.437, readyState: 0, duration: 0 };
  const metadataReset = { ...resumed, currentTime: 1.416, startPosition: 349.437, duration: 1.416 };
  expect(hasWebviewStartupResumeProgress(optimisticAssignment)).toBe(false);
  expect(hasWebviewStartupResumeProgress(metadataReset)).toBe(false);
  expect(hasWebviewStartupResumeProgress({ ...resumed, currentTime: 350, duration: 1.416 })).toBe(false);
});

it("blocks history only for the exact unverified startup-resume owner", () => {
  const node = {}, source = {};
  const gate = { node, source, trackId: "track-a", verified: false };
  expect(startupResumeBlocksHistory(gate, node, source, "track-a")).toBe(true);
  expect(startupResumeBlocksHistory({ ...gate, verified: true }, node, source, "track-a")).toBe(false);
  expect(startupResumeBlocksHistory(gate, {}, source, "track-a")).toBe(false);
  expect(startupResumeBlocksHistory(gate, node, source, "track-b")).toBe(false);
  expect(startupResumeBlocksHistory(null, node, source, "track-a")).toBe(false);
});
