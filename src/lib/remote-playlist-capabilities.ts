export type PlaylistWriteCapabilities = { add: boolean; remove: boolean; reorder: boolean };
export function playlistWriteCapabilities(canEdit: boolean | undefined, explicit: unknown): PlaylistWriteCapabilities {
  if (explicit === undefined) return { add: canEdit === true, remove: canEdit === true, reorder: canEdit === true };
  if (explicit && typeof explicit === "object" && !Array.isArray(explicit)) {
    const row = explicit as Record<string, unknown>;
    if (Object.keys(row).length === 3 && ["add", "remove", "reorder"].every(key => typeof row[key] === "boolean")) {
      return { add: row.add as boolean, remove: row.remove as boolean, reorder: row.reorder as boolean };
    }
  }
  return { add: false, remove: false, reorder: false };
}
export function hasPlaylistWriteCapability(canEdit: boolean | undefined, explicit: unknown): boolean {
  const caps = playlistWriteCapabilities(canEdit, explicit);
  return caps.add || caps.remove || caps.reorder;
}
export function playlistWritePlanAllowed(canEdit: boolean | undefined, explicit: unknown, changes: unknown): boolean {
  if (!changes || typeof changes !== "object") return false;
  const plan = changes as Record<string, unknown>;
  if (!["add", "remove", "move"].every(key => Array.isArray(plan[key]))) return false;
  const caps = playlistWriteCapabilities(canEdit, explicit);
  return (caps.add || caps.remove || caps.reorder) && (!(plan.add as unknown[]).length || caps.add) && (!(plan.remove as unknown[]).length || caps.remove)
    && (!(plan.move as unknown[]).length || caps.reorder);
}
