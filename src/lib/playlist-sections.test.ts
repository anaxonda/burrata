import { describe, expect, it } from "vitest";
import {
  groupPlaylistsBySection,
  movePlaylistAssignment,
  PLAYLIST_SECTION_DRAG_TYPE,
  readDraggedPlaylistId,
} from "./playlist-sections";

const sections = [{ id: "workout", name: "Workout" }, { id: "late-night", name: "Late Night" }];
const playlists = [
  { id: "p1", name: "House", trackCount: 12 },
  { id: "p2", name: "Jazz", trackCount: 4 },
  { id: "p3", name: "Ambient", trackCount: 2 },
];

describe("playlist sections data", () => {
  it("groups each playlist once and falls back to unassigned when its section was deleted", () => {
    const groups = groupPlaylistsBySection(sections, { p1: "workout", p2: "removed-section", stale: "late-night" }, playlists);
    expect(groups.map(group => [group.sectionId, group.playlists.map(item => item.id)])).toEqual([
      [null, ["p2", "p3"]], ["workout", ["p1"]], ["late-night", []],
    ]);
    expect(groups.flatMap(group => group.playlists.map(item => item.id)).sort()).toEqual(["p1", "p2", "p3"]);
  });

  it("moves only known playlists to known sections and removes assignments for the unassigned drop target", () => {
    expect(movePlaylistAssignment({ p1: "workout" }, "p1", "late-night", sections, playlists)).toEqual({ p1: "late-night" });
    expect(movePlaylistAssignment({ p1: "workout" }, "p1", null, sections, playlists)).toEqual({});
    expect(movePlaylistAssignment({}, "missing", "workout", sections, playlists)).toBeNull();
    expect(movePlaylistAssignment({}, "p1", "missing", sections, playlists)).toBeNull();
    expect(movePlaylistAssignment({ p1: "workout" }, "p1", "workout", sections, playlists)).toBeNull();
  });

  it("accepts only internal drag data naming an existing playlist", () => {
    const good: DataTransfer = {
      types: [PLAYLIST_SECTION_DRAG_TYPE],
      getData: (type: string) => type === PLAYLIST_SECTION_DRAG_TYPE ? "p2" : "",
    } as unknown as DataTransfer;
    expect(readDraggedPlaylistId(good, playlists)).toBe("p2");
    expect(readDraggedPlaylistId({ types: ["text/plain"], getData: () => "p2" } as unknown as DataTransfer, playlists)).toBeNull();
    expect(readDraggedPlaylistId({ types: [PLAYLIST_SECTION_DRAG_TYPE], getData: () => "external-id" } as unknown as DataTransfer, playlists)).toBeNull();
    expect(readDraggedPlaylistId({ types: [PLAYLIST_SECTION_DRAG_TYPE], getData: () => { throw new Error("blocked"); } } as unknown as DataTransfer, playlists)).toBeNull();
  });
});
