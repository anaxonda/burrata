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

it("uses the full account-scoped album's ordered track list for queue and playlist actions", async () => {
  const target = document.createElement("div");
  document.body.append(target);
  const tracks = Array.from({ length: 101 }, (_, index) => ({
    id: `subsonic:track-${index}`, provider: "subsonic", title: `Track ${index + 1}`,
    artist: "Performer", albumArtist: "Performer", trackNumber: index + 1, album: "Edition",
  }));
  let refreshed = false;
  const call = vi.fn(async (command, args: { offset: number; limit: number }) => command === "browse_subsonic"
    ? { tracks: tracks.slice(args.offset, args.offset + args.limit), total: tracks.length, snapshotId: refreshed ? "new-catalog" : "original-catalog" }
    : null);
  const onQueue = vi.fn(), onAddAlbumToPlaylist = vi.fn();
  const view = mount(SubsonicLibrary, { target, props: {
    call, status: { trackCount: 101 }, scope: { kind: "album", trackId: tracks[0].id, label: "Edition" },
    favoriteIds: [], onStatus: vi.fn(), onPlay: vi.fn(), onFavorite: vi.fn(), onPlaylist: vi.fn(),
    onQueue, onAddAlbumToPlaylist,
  } as never });
  try {
    await vi.waitFor(() => expect(target.querySelector('.album-detail h1')?.textContent).toBe("Edition"));
    const button = (label: string) => [...target.querySelectorAll("button")].find(node => node.textContent?.trim() === label)!;
    await vi.waitFor(() => expect(button("＋ Queue album").disabled).toBe(false));
    button("＋ Queue album").click();
    await vi.waitFor(() => expect(onQueue).toHaveBeenCalledOnce());
    expect(onQueue.mock.calls[0][0].map((row: { id: string }) => row.id)).toEqual(tracks.map(row => row.id));
    expect(call).toHaveBeenCalledWith("browse_subsonic", { query: "", offset: 100, limit: 100, scope: { kind: "album", trackId: tracks[0].id } });
    button("Add to playlist").click();
    await vi.waitFor(() => expect(onAddAlbumToPlaylist).toHaveBeenCalledOnce());
    expect(onAddAlbumToPlaylist.mock.calls[0][0]).toHaveLength(101);
    expect(onAddAlbumToPlaylist.mock.calls[0][1]).toBe("Edition");
    refreshed = true;
    button("＋ Queue album").click();
    await vi.waitFor(() => expect(target.textContent).toContain("The Subsonic album changed while loading. Try again."));
    expect(onQueue).toHaveBeenCalledOnce();
  } finally {
    await unmount(view);
    target.remove();
  }
});
