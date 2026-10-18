/** Search history must not turn a scope check into provider status/login work. */
export const publicSearchNavigationScope = (id: string): string | null =>
  id === "spotify" || id === "youtube" || id === "soulseek" ? null : `public:${id}`;
export async function searchNavigationScopes(
  call: (command: string, args: Record<string, unknown>) => Promise<unknown>,
  ids: readonly string[],
): Promise<Record<string, string | null>> {
  const scopes = Object.fromEntries(ids.map(id => [id, publicSearchNavigationScope(id)]));
  if (!ids.some(id => id === "spotify" || id === "youtube")) return scopes;
  try {
    //Local state ONLY. remote_playlist_connection_status can dispatch jobs.
    const value = await call("get_managed_provider_session_status", {}) as {
      enabled?: boolean; running?: boolean; mode?: string;
      accountRevisions?: Record<string, number>;
      spotifyAccountId?: string | null; youtubeAccountId?: string | null;
      youtubeReconnectRequired?: boolean;
    } | null;
    if (!value || typeof value !== "object") return scopes;
    const unavailable = value.enabled === false || value.running === false || value.mode === "stopped" || value.mode === "native-confirmed";
    for (const id of ids) {
      if (id !== "spotify" && id !== "youtube") continue;
      const revision = value.accountRevisions?.[id];
      if (!Number.isSafeInteger(revision) || revision! < 0 || id === "youtube" && value.youtubeReconnectRequired) continue;
      const account = id === "spotify" ? value.spotifyAccountId : value.youtubeAccountId;
      if (typeof account === "string" && account.trim() && account.length <= 512) {
        if (!unavailable) scopes[id] = `${id}:revision:${revision}:${account}`;
      } else if (id === "youtube" && (account === null || account === undefined)) {
        //Anonymous public search does not need login. Revision binds cursors to
        //that local identity epoch and changes on subsequent sign-in/out.
        scopes[id] = `youtube:public:revision:${revision}`;
      }
    }
  } catch { /* Unknown scope disables reuse, never invokes a provider fallback. */ }
  return scopes;
}
