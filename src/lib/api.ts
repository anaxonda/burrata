/** Tauri's supported global API, enabled by app.withGlobalTauri. */
declare global {
  interface Window {
    __TAURI__?: {
      core: {
        invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
        convertFileSrc(path: string, protocol?: string): string;
      };
      event: {
        listen<T>(
          event: string,
          handler: (event: { event: string; id: number; payload: T }) => void,
        ): Promise<() => void>;
      };
    };
  }
}
export const desktop = (): boolean => Boolean(window.__TAURI__);
export async function call<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (!window.__TAURI__)
    throw new Error(
      "Open Burrata as a desktop application to access providers and your library.",
    );
  return window.__TAURI__.core.invoke<T>(command, args);
}
export async function listen<T>(
  event: string,
  handler: (event: { event: string; id: number; payload: T }) => void,
): Promise<() => void> {
  if (!window.__TAURI__)
    throw new Error("Desktop event connection is unavailable.");
  return window.__TAURI__.event.listen<T>(event, handler);
}
export function playable(url: string): string {
  if (/^(https?:|blob:|data:|asset:)/.test(url)) return url;
  if (!window.__TAURI__)
    throw new Error("Local playback requires the desktop application.");
  return window.__TAURI__.core.convertFileSrc(url);
}
