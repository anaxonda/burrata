import { describe, expect, it } from "vitest";
import { savedLocalPlaylistContext } from "./download-playlist-context";

describe("savedLocalPlaylistContext", () => {
  const collections = [
    { id: "saved-1", name: "Road Trip" },
    { id: "imported-snapshot", name: "Imported chart snapshot" },
    { id: "synced-local", name: "Synced playlist copy" },
    { id: "saved-2", name: "   " },
  ];

  it("captures the exact saved playlist ID and trimmed name", () => {
    expect(savedLocalPlaylistContext(collections, "saved-1")).toEqual({ id: "saved-1", name: "Road Trip" });
  });

  it("accepts persisted imported and synced snapshots as local saved collections", () => {
    expect(savedLocalPlaylistContext(collections, "imported-snapshot")).toEqual({ id: "imported-snapshot", name: "Imported chart snapshot" });
    expect(savedLocalPlaylistContext(collections, "synced-local")).toEqual({ id: "synced-local", name: "Synced playlist copy" });
  });

  it("does not infer a playlist context for an unsaved remote ID or blank collection", () => {
    expect(savedLocalPlaylistContext(collections, "missing")).toBeUndefined();
    expect(savedLocalPlaylistContext(collections, "remote-source-123")).toBeUndefined();
    expect(savedLocalPlaylistContext(collections, "saved-2")).toBeUndefined();
  });
});
