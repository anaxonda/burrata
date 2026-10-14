import { describe, expect, it } from "vitest";
import { playbackAction } from "./playback-action";
describe("keyboard auditioning with selection separate from playback", () => {
  it("starts the newly selected recording after ArrowDown then Space while prior audio still plays", () => {
    expect(playbackAction("next-chart-row", "previous-preview", true)).toBe(
      "start-selected",
    );
  });
  it("toggles playback when selection and playing recording are the same", () => {
    expect(playbackAction("same-recording", "same-recording", true)).toBe(
      "toggle-current",
    );
  });
  it("keeps persistent player controls useful when a different view has no selection", () => {
    expect(playbackAction(undefined, "previous-preview", true)).toBe(
      "toggle-current",
    );
    expect(playbackAction(undefined, undefined, false)).toBe("none");
  });
});
