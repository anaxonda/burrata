import { describe, expect, it } from "vitest";
import { allTrackSortFieldsAvailable, trackSortAvailability, trackSortAvailabilityForSources, trackSortOptionLabel, trackSortOptionLabelForSources } from "./track-sort-availability";
import type { Track } from "./types";

function track(fields: Partial<Track> = {}): Track {
  return { id: "1", provider: "test", providerId: "1", sourceUrl: "", artist: "", title: "", favorite: false, provenance: {}, ...fields };
}

describe("track sort availability", () => {
  it("keeps source order available for empty data and disables empty value fields", () => {
    const availability = trackSortAvailability([]);
    expect(availability.sourceOrder).toBe(true);
    expect(availability.title).toBe(false);
    expect(trackSortOptionLabel(availability, "title", "Title")).toBe("Title (unavailable)");
  });

  it("disables fields only when every current row lacks a comparable value", () => {
    const availability = trackSortAvailability([
      track({ title: "  ", durationSeconds: 0, provenance: { plays: "unknown" } }),
      track({ title: "Song", provenance: { views: 0 } }),
    ]);
    expect(availability.title).toBe(true);
    expect(availability.duration).toBe(false);
    expect(availability.views).toBe(true);
    expect(availability.plays).toBe(false);
  });

  it("can defer availability when unseen pages may contain values", () => {
    expect(allTrackSortFieldsAvailable().bpm).toBe(true);
  });
});


describe("source-aware track sort availability", () => {
  it("keeps unknown provider capabilities available while pages are incomplete", () => {
    const availability = trackSortAvailabilityForSources([track({ provider: "spotify" })], ["spotify"], true);
    expect(availability.title).toBe(true);
    expect(availability.plays).toBe(true);
    expect(availability.views).toBe(true);
    expect(availability.bpm).toBe(true);
    expect(trackSortOptionLabelForSources(availability, "bpm", "BPM", ["spotify"]))
      .toBe("BPM");
  });

  it("keeps sort defaults and treats generic Spotify and YouTube as unknown", () => {
    const availability = trackSortAvailabilityForSources([], ["spotify", "youtube"], true);
    expect(availability.sourceOrder).toBe(true);
    expect(availability.title).toBe(true);
    expect(availability.views).toBe(true);
    expect(availability.plays).toBe(true);
    expect(availability.bpm).toBe(true);
  });

  it("disables only fields excluded by an exact parser operation", () => {
    const availability = trackSortAvailabilityForSources([], ["lastfm-top-tracks"], true);
    expect(availability.title).toBe(true);
    expect(availability.artist).toBe(true);
    expect(availability.album).toBe(false);
    expect(availability.plays).toBe(false);
    expect(trackSortOptionLabelForSources(availability, "plays", "Plays", ["lastfm-top-tracks"]))
      .toBe("Plays (not supported by Last.fm top tracks)");
  });

  it("lets observed fields remain enabled even with a parser capability declaration", () => {
    const availability = trackSortAvailabilityForSources(
      [track({ provider: "lastfm", provenance: { views: 10 } })], ["lastfm-top-tracks"], false,
    );
    expect(availability.views).toBe(true);
    expect(trackSortOptionLabelForSources(availability, "views", "Views", ["lastfm-top-tracks"]))
      .toBe("Views");
  });

  it("does not infer unsupported capabilities for unknown providers", () => {
    const availability = trackSortAvailabilityForSources([], ["new-provider"], true);
    expect(availability.sourceOrder).toBe(true);
    expect(availability.bpm).toBe(true);
    expect(trackSortOptionLabelForSources(availability, "bpm", "BPM", ["new-provider"]))
      .toBe("BPM");
  });

  it("uses observed values for complete datasets and preserves ordinary unavailable labels", () => {
    const availability = trackSortAvailabilityForSources([track({ provider: "spotify", title: "Track" })], ["spotify"]);
    expect(availability.title).toBe(true);
    expect(availability.duration).toBe(false);
    expect(availability.bpm).toBe(false);
    expect(trackSortOptionLabel(availability, "bpm", "BPM")).toBe("BPM (unavailable)");
    expect(availability.sourceOrder).toBe(true);
  });
});
