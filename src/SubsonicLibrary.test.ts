// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, unmount } from "../node_modules/svelte/src/index-client.js";
import { expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime lacks public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import SubsonicLibrary from "./SubsonicLibrary.svelte";

it("renders catalog tracks with the Library's array of favorite IDs", async () => {
  const target = document.createElement("div");
  document.body.append(target);
  const call = vi.fn(async () => ({
    tracks: [
      { id: "subsonic:test:one", provider: "subsonic", artist: "Example", title: "First" },
      { id: "subsonic:test:two", provider: "subsonic", artist: "Example", title: "Second" },
    ],
    total: 2,
  }));
  const view = mount(SubsonicLibrary, {
    target,
    props: {
      call,
      status: { trackCount: 2 },
      favoriteIds: ["subsonic:test:one"],
      onStatus: vi.fn(),
      onPlay: vi.fn(),
      onFavorite: vi.fn(),
      onPlaylist: vi.fn(),
      onQueue: vi.fn(),
    } as never,
  });
  try {
    await vi.waitFor(() => {
      expect(target.querySelector('[aria-label="Remove First from favorites"]')).not.toBeNull();
      expect(target.querySelector('[aria-label="Add Second to favorites"]')).not.toBeNull();
    });
    expect(call).toHaveBeenCalledWith("browse_subsonic", { query: "", offset: 0, limit: 100 });
  } finally {
    await unmount(view);
    target.remove();
  }
});
