import type { PlaylistContext } from "./types";

export interface SavedPlaylistDescriptor {
  id: string;
  name: string;
}

/** Return folder-layout context only when the requested ID names a saved local playlist. */
export function savedLocalPlaylistContext(
  collections: readonly SavedPlaylistDescriptor[],
  id: string,
): PlaylistContext | undefined {
  if (!id) return undefined;
  const collection = collections.find(item => item.id === id);
  const name = collection?.name.trim();
  return collection && name ? { id: collection.id, name } : undefined;
}
