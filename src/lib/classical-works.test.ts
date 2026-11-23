import { describe, expect, it } from "vitest";
import { classicalAlbumProgrammes, classicalWorkPlacement, findClassicalWorks } from "./classical-works";
import type { Track } from "./types";

const id = (number: number) => `00000000-0000-4000-8000-${String(number).padStart(12, "0")}`;

function catalog(part: number, position = part, event = 3, conductor = 4): Track {
  const release = id(1), recording = id(100 + position);
  const providerId = `musicbrainz:${release}:1:${position}:1:${recording}`;
  return {
    id: `catalog:${providerId}`, providerId, provider: "catalog", sourceUrl: `https://musicbrainz.org/release/${release}`,
    title: `Movement ${part}`, artist: "Orchestra", album: "The symphonies", favorite: false,
    provenance: {
      sourceKind: "catalog", catalogProvider: "musicbrainz", releaseId: release, recordingId: recording,
      classicalWork: {
        id: id(20 + part), parentId: id(2), title: "Symphony No. 5, Op. 67", movement: `Movement ${part}`, classical: true,
        composers: [{ id: id(5), name: "Lúdwíg van Beethoven" }], partNumber: part, partTotal: 2,
        performanceEventId: event ? id(event) : null,
      },
      recordingRelations: [
        { type: "orchestra", artist: { id: id(6), name: "Orchestra" } },
        { type: "conductor", artist: { id: id(conductor), name: `Conductor ${conductor}` } },
      ],
    },
  };
}

function local(part: number, localPath = `/music/album/${part}.flac`): Track {
  return {
    id: `local:${localPath}`, provider: "local", providerId: localPath, sourceUrl: "", localPath,
    title: `Mov. ${part}`, artist: "Soloist", album: "The sonatas", favorite: false,
    provenance: {
      source: "local file tags",
      embeddedClassical: {
        version: 1, work: "Sonata", workId: id(40 + part), movement: `Mov. ${part}`, movementNumber: part,
        movementTotal: 2, composers: ["Composer"],
      },
    },
  };
}

describe("classical album programmes", () => {
  it("groups the same work by evidenced performance and preserves exact album occurrences and movement order", () => {
    const tracks = [catalog(1), catalog(2), catalog(1, 3, 7), catalog(2, 4, 7)];
    const groups = classicalAlbumProgrammes(tracks);
    expect(groups.map(group => [group.startIndex, group.complete])).toEqual([[0, true], [2, true]]);
    expect(groups[0]?.tracks).toEqual(tracks.slice(0, 2));
    expect(groups[0]?.tracks[0]).toBe(tracks[0]);
    expect(groups[1]?.tracks).toEqual(tracks.slice(2));
  });

  it("keeps separate performances, unverified sources, gaps and manual edits as individual recordings", () => {
    expect(classicalAlbumProgrammes([catalog(1, 1, 0), catalog(2, 2, 0)])).toEqual([]);
    expect(classicalAlbumProgrammes([catalog(1, 1, 3), catalog(2, 2, 7)])).toEqual([]);
    expect(classicalAlbumProgrammes([catalog(1, 1, 3, 4), catalog(2, 2, 3, 8)])).toEqual([]);
    expect(classicalAlbumProgrammes([catalog(2, 2), catalog(1, 1)])).toEqual([]);
    expect(classicalAlbumProgrammes([catalog(1, 1), catalog(3, 2)])).toEqual([]);
    const first = catalog(1);
    expect(classicalWorkPlacement({ ...first, provenance: { ...first.provenance as object, manualFields: { title: true } } })).toBeNull();
    expect(classicalWorkPlacement({ ...first, sourceUrl: "https://example.test/release" })).toBeNull();
    expect(classicalWorkPlacement({ ...first, provider: "import" })).toBeNull();
  });

  it("accepts complete authored movement tags within one local album and never codes them as catalog proof", () => {
    const tracks = [local(1), local(2)];
    const [group] = classicalAlbumProgrammes(tracks);
    expect(group?.source).toBe("fileTags");
    expect(group?.tracks).toEqual(tracks);
    expect(classicalAlbumProgrammes([local(1), local(2, "/music/another/2.flac")])).toEqual([]);
    expect(classicalAlbumProgrammes([local(1), { ...local(2), artist: "Another soloist" }])).toEqual([]);
  });
});

describe("Works search", () => {
  it("uses the parent work identity and finds it by composer, title or catalog number", () => {
    const rows = [catalog(1), catalog(2), catalog(1, 3, 7), catalog(2, 4, 7), local(1)];
    for (const query of ["beethoven", "symphony 5", "Op. 67", "op 67 beethoven"]) {
      expect(findClassicalWorks(rows, query)).toEqual([
        expect.objectContaining({ key: `musicbrainz:${id(2)}`, tracks: rows.slice(0, 4) }),
      ]);
    }
    expect(findClassicalWorks(rows, "Sonata")).toEqual([expect.objectContaining({ source: "fileTags", title: "Sonata" })]);
    expect(findClassicalWorks(rows, "Concerto")).toEqual([]);
  });
});
