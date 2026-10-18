import { describe, expect, it } from "vitest";
import { trackFileGroupsByTrackId } from "./track-file-groups";

describe("track file groups", () => {
  it("keeps catalog aliases mapped to every canonical recording file", () => {
    const files = [{ id: "file-a", recordingId: "recording-1" }, { id: "file-b", recordingId: "recording-1" }];
    const byTrack = trackFileGroupsByTrackId([{ trackId: "catalog-alias", files }]);
    expect(byTrack["catalog-alias"]).toEqual(files);
    expect(byTrack["recording-1"]).toBeUndefined();
  });
});
