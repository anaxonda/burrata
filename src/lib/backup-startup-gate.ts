/** Do not initialize playback or background work until critical restore hydration succeeds. */
export async function startAfterBackupHydration(
  hydrate: () => Promise<boolean>,
  initialize: () => void,
): Promise<boolean> {
  if (!(await hydrate())) return false;
  initialize();
  return true;
}
