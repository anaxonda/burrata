export type PlaylistSection = { id: string; name: string };
export type PlaylistCollection = { id: string; name: string; trackCount: number };
export type PlaylistSectionAssignments = Record<string, string>;
export const PLAYLIST_SECTION_DRAG_TYPE = "application/x-burrata-playlist-id";

export function normalizeSectionAssignments(
  sections: PlaylistSection[],
  assignments: PlaylistSectionAssignments,
  playlists: Pick<PlaylistCollection, "id">[],
): PlaylistSectionAssignments {
  const validSectionIds = new Set(sections.map((section) => section.id));
  const validPlaylistIds = new Set(playlists.map((playlist) => playlist.id));
  const normalized: PlaylistSectionAssignments = {};
  for (const [playlistId, sectionId] of Object.entries(assignments)) {
    if (validPlaylistIds.has(playlistId) && validSectionIds.has(sectionId)) normalized[playlistId] = sectionId;
  }
  return normalized;
}

export function groupPlaylistsBySection(
  sections: PlaylistSection[],
  assignments: PlaylistSectionAssignments,
  playlists: PlaylistCollection[],
): { sectionId: string | null; playlists: PlaylistCollection[] }[] {
  const validAssignments = normalizeSectionAssignments(sections, assignments, playlists);
  return [
    { sectionId: null, playlists: playlists.filter((playlist) => !validAssignments[playlist.id]) },
    ...sections.map((section) => ({
      sectionId: section.id,
      playlists: playlists.filter((playlist) => validAssignments[playlist.id] === section.id),
    })),
  ];
}

export function movePlaylistAssignment(
  assignments: PlaylistSectionAssignments,
  playlistId: string,
  targetSectionId: string | null,
  sections: PlaylistSection[],
  playlists: Pick<PlaylistCollection, "id">[],
): PlaylistSectionAssignments | null {
  if (!playlists.some((playlist) => playlist.id === playlistId)) return null;
  const validSectionIds = new Set(sections.map((section) => section.id));
  if (targetSectionId !== null && !validSectionIds.has(targetSectionId)) return null;
  const normalized = normalizeSectionAssignments(sections, assignments, playlists);
  const current = normalized[playlistId] ?? null;
  if (current === targetSectionId) return null;
  if (targetSectionId === null) delete normalized[playlistId];
  else normalized[playlistId] = targetSectionId;
  return normalized;
}

export function readDraggedPlaylistId(
  transfer: Pick<DataTransfer, "getData" | "types">,
  playlists: Pick<PlaylistCollection, "id">[],
): string | null {
  try {
    if (!Array.from(transfer.types).includes(PLAYLIST_SECTION_DRAG_TYPE)) return null;
    const id = transfer.getData(PLAYLIST_SECTION_DRAG_TYPE);
    return playlists.some((playlist) => playlist.id === id) ? id : null;
  } catch {
    return null;
  }
}
