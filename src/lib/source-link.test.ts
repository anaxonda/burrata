import { describe, expect, it } from "vitest";
import { trackSource } from "./source-link";
import type { Track } from "./types";

const track = (sourceUrl: string, provenance: unknown = {}) => ({ sourceUrl, provenance } as Track);
describe("original track source", () => {
  it("keeps the Beatport catalog URL rather than a preview or acquired source", () => {
    expect(trackSource({ ...track("https://www.beatport.com/track/song/123"),
      previewUrl: "https://cdn.example.com/preview.mp3", localPath: "/music/song.mp3" })).toEqual({
      url: "https://www.beatport.com/track/song/123", label: "Beatport",
    });
  });
  it("supports track links and falls back to the captured chart URL", () => {
    expect(trackSource(track("https://www.traxsource.com/track/123/song"))?.label).toBe("Traxsource");
    expect(trackSource(track("https://open.spotify.com/track/123"))?.label).toBe("Spotify");
    expect(trackSource(track("urn:import:123", { sourceUrl: "https://www.beatport.com/genre/afro-house/89/top-100" }))?.label).toBe("Beatport");
  });
  it("does not make external actions from local or unsafe references", () => {
    for (const url of ["file:///etc/passwd", "javascript:alert(1)", "http://localhost/x", "https://127.0.0.1/x", "https://user:pass@example.com/x", "urn:import:123"])
      expect(trackSource(track(url))).toBeNull();
  });
});
