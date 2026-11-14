import type { Track } from "./types";

/** Small sequential reads; navigation away stops the remaining work. */
export async function readLibraryTrackMembership(
  tracks: Track[],
  call: <T>(command: string, args: Record<string, unknown>) => Promise<T>,
  current: () => boolean,
): Promise<Set<string>> {
  const saved = new Set<string>();
  for (let offset = 0; offset < tracks.length && current(); offset += 200) {
    const batch = tracks.slice(offset, offset + 200);
    const ids = new Set(batch.map(track => track.id));
    const result = await call<string[]>("get_library_track_membership", { tracks: batch });
    if (!current()) break;
    if (!Array.isArray(result)) throw new Error("Library status unavailable");
    for (const id of result) if (ids.has(id)) saved.add(id);
  }
  return saved;
}
