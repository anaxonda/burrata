/** Preserve the periodic sync only when the native watcher is unavailable. */
export async function runPeriodicPlaylistSyncFallback(
  watcherHealthy: () => Promise<boolean>,
  sync: () => void | Promise<void>,
): Promise<boolean> {
  try {
    if (await watcherHealthy()) return false;
  } catch {
    // Older builds or a failed health check conservatively use the fallback.
  }
  await sync();
  return true;
}
