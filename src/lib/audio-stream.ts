type HlsErrorData = { fatal?: boolean; details?: string; type?: string };

type HlsInstance = {
  on(event: string, listener: (event: string, data?: HlsErrorData) => void): void;
  off(event: string, listener: (event: string, data?: HlsErrorData) => void): void;
  attachMedia(media: HTMLMediaElement): void;
  loadSource(url: string): void;
  destroy(): void;
};

type HlsConstructor = {
  new (config?: Record<string, unknown>): HlsInstance;
  isSupported(): boolean;
  Events: { MEDIA_ATTACHED: string; ERROR: string };
};

export type AudioStreamKind = "direct" | "native-hls" | "mse-hls";
export type AudioStreamHandle = { kind: AudioStreamKind; destroy(): void };

export type AudioStreamOptions = {
  isCurrent?: () => boolean;
  onError?: (error: Error) => void;
  /** Injectable loader keeps the lifecycle testable without duplicating the adapter. */
  loadHls?: () => Promise<HlsConstructor>;
};

const audioOwners = new WeakMap<HTMLAudioElement, object>();
const HLS_MIME_TYPES = ["application/vnd.apple.mpegurl", "application/x-mpegurl"];

function isHlsUrl(url: string, mimeType?: string | null): boolean {
  const mime = (mimeType ?? "").split(";", 1)[0]?.trim().toLowerCase();
  if (mime && HLS_MIME_TYPES.includes(mime)) return true;
  try {
    return new URL(url, globalThis.location?.href).pathname.toLowerCase().endsWith(".m3u8");
  } catch {
    return url.split(/[?#]/, 1)[0]?.toLowerCase().endsWith(".m3u8") ?? false;
  }
}

function abortError(): Error {
  const error = new Error("Audio stream setup was cancelled.");
  error.name = "AbortError";
  return error;
}

function setDirectSource(audio: HTMLAudioElement, url: string, kind: "direct" | "native-hls"): AudioStreamHandle {
  const owner = {};
  audioOwners.set(audio, owner);
  audio.src = url;
  audio.load();
  return {
    kind,
    destroy() {
      if (audioOwners.get(audio) !== owner) return;
      audioOwners.delete(audio);
      audio.pause();
      audio.removeAttribute("src");
      audio.load();
    },
  };
}

async function defaultLoadHls(): Promise<HlsConstructor> {
  const module = await import("hls.js");
  return module.default as unknown as HlsConstructor;
}

/** Attach a direct audio URL or prepare HLS with native playback preferred over MSE. */
export async function attachAudioStream(
  audio: HTMLAudioElement,
  url: string,
  mimeType?: string | null,
  options: AudioStreamOptions = {},
): Promise<AudioStreamHandle> {
  const isCurrent = options.isCurrent ?? (() => true);
  if (!isCurrent()) throw abortError();
  if (!isHlsUrl(url, mimeType)) return setDirectSource(audio, url, "direct");

  const canPlayNative = HLS_MIME_TYPES.some((mime) => {
    try { return Boolean(audio.canPlayType(mime)); } catch { return false; }
  });
  if (canPlayNative) return setDirectSource(audio, url, "native-hls");

  const Hls = await (options.loadHls ?? defaultLoadHls)();
  if (!isCurrent()) throw abortError();
  if (!Hls.isSupported()) throw new Error("This browser cannot play HLS audio streams.");

  return new Promise<AudioStreamHandle>((resolve, reject) => {
    const owner = {};
    audioOwners.set(audio, owner);
    audio.pause();
    audio.removeAttribute("src");
    audio.load();
    const hls = new Hls();
    let settled = false;
    let destroyed = false;
    let fatalForwarded = false;
    let staleCheck: ReturnType<typeof setInterval> | undefined;

    const releaseOwnedAudio = () => {
      if (audioOwners.get(audio) !== owner) return;
      audioOwners.delete(audio);
      audio.pause();
      audio.removeAttribute("src");
      audio.load();
    };

    const destroy = () => {
      if (destroyed) return;
      destroyed = true;
      if (staleCheck) clearInterval(staleCheck);
      hls.off(Hls.Events.MEDIA_ATTACHED, onAttached);
      hls.off(Hls.Events.ERROR, onHlsError);
      hls.destroy();
      if (!settled) {
        settled = true;
        reject(abortError());
      }
      releaseOwnedAudio();
    };
    const onAttached = () => {
      if (destroyed) return;
      if (!isCurrent()) { destroy(); return; }
      try {
        hls.loadSource(url);
        settled = true;
        if (staleCheck) clearInterval(staleCheck);
        resolve({ kind: "mse-hls", destroy });
      } catch (cause) {
        const error = cause instanceof Error ? cause : new Error(String(cause));
        destroyed = true;
        if (staleCheck) clearInterval(staleCheck);
        hls.off(Hls.Events.MEDIA_ATTACHED, onAttached);
        hls.off(Hls.Events.ERROR, onHlsError);
        hls.destroy();
        releaseOwnedAudio();
        settled = true;
        reject(error);
      }
    };
    const onHlsError = (_event: string, data?: HlsErrorData) => {
      if (!data?.fatal || fatalForwarded || destroyed) return;
      fatalForwarded = true;
      const error = new Error(data.details || `Fatal HLS playback error${data.type ? ` (${data.type})` : ""}.`);
      if (!settled) {
        destroyed = true;
        hls.off(Hls.Events.MEDIA_ATTACHED, onAttached);
        hls.off(Hls.Events.ERROR, onHlsError);
        hls.destroy();
        if (staleCheck) clearInterval(staleCheck);
        releaseOwnedAudio();
        settled = true;
        reject(error);
      } else {
        options.onError?.(error);
        audio.dispatchEvent(new Event("error"));
        destroy();
      }
    };

    hls.on(Hls.Events.MEDIA_ATTACHED, onAttached);
    hls.on(Hls.Events.ERROR, onHlsError);
    staleCheck = setInterval(() => { if (!isCurrent()) destroy(); }, 50);
    try { hls.attachMedia(audio); }
    catch (cause) {
      const error = cause instanceof Error ? cause : new Error(String(cause));
      hls.off(Hls.Events.MEDIA_ATTACHED, onAttached);
      hls.off(Hls.Events.ERROR, onHlsError);
      hls.destroy();
      if (staleCheck) clearInterval(staleCheck);
      releaseOwnedAudio();
      settled = true;
      reject(error);
    }
  });
}
