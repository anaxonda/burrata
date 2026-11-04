import { describe, expect, it } from "vitest";
import { projectPlaylistMembershipsForDisplay } from "./track-library-summary";

describe("projectPlaylistMembershipsForDisplay", () => {
  it("shows a same-name linked working copy once when both memberships exist", () => {
    // Synthetic IDs representing the reported imported eight-track pair.
    const memberships = [
      { id: "source-snapshot", name: "Shared playlist" },
      { id: "local-working-copy", name: "Shared playlist" },
    ];
    expect(projectPlaylistMembershipsForDisplay(memberships, [
      { sourceCollectionId: memberships[0].id, collectionId: memberships[1].id },
    ])).toEqual([memberships[1]]);
  });

  it("preserves unrelated same-name playlists", () => {
    const memberships = [{ id: "a", name: "World war clarinet" }, { id: "b", name: "World war clarinet" }];
    expect(projectPlaylistMembershipsForDisplay(memberships, [])).toEqual(memberships);
  });

  it("keeps source-only membership when the working copy lacks the track", () => {
    const memberships = [{ id: "source", name: "Shared name" }];
    expect(projectPlaylistMembershipsForDisplay(memberships, [
      { sourceCollectionId: "source", collectionId: "working" },
    ])).toEqual(memberships);
  });

  it("keeps renamed bound copies distinct in the membership list", () => {
    const memberships = [{ id: "source", name: "Imported name" }, { id: "working", name: "My edited copy" }];
    expect(projectPlaylistMembershipsForDisplay(memberships, [
      { sourceCollectionId: "source", collectionId: "working" },
    ])).toEqual(memberships);
  });

  it("does not merge without the explicit source-to-copy binding", () => {
    const memberships = [{ id: "source", name: "Shared name" }, { id: "working", name: "Shared name" }];
    expect(projectPlaylistMembershipsForDisplay(memberships, [
      { collectionId: "working" },
    ])).toEqual(memberships);
  });

  it("keeps chained working copies visible while hiding only the original snapshot", () => {
    const memberships = ["source", "working-b", "working-c"].map(id => ({ id, name: "Shared name" }));
    expect(projectPlaylistMembershipsForDisplay(memberships, [
      { sourceCollectionId: "source", collectionId: "working-b" },
      { sourceCollectionId: "working-b", collectionId: "working-c" },
    ])).toEqual(memberships.slice(1));
  });

  it("keeps both memberships visible for cyclic bindings", () => {
    const memberships = [{ id: "a", name: "Shared name" }, { id: "b", name: "Shared name" }];
    expect(projectPlaylistMembershipsForDisplay(memberships, [
      { sourceCollectionId: "a", collectionId: "b" },
      { sourceCollectionId: "b", collectionId: "a" },
    ])).toEqual(memberships);
  });
});
