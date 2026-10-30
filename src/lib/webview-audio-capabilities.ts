/** Codec availability belongs to the actual WebView, rather than an OS guess. */
export function webviewAudioCapabilities(node?: Pick<HTMLAudioElement, "canPlayType">) {
  const audio = node ?? (typeof document === "undefined" ? undefined : document.createElement("audio"));
  const supports = (mime: string) => { try { return Boolean(audio?.canPlayType(mime)); } catch { return false; } };
  return { oggVorbis: supports('audio/ogg; codecs="vorbis"'), oggOpus: supports('audio/ogg; codecs="opus"') };
}

/** Each WebView needs one media-registry initialization, not one per source. */
export function createWebviewAudioCapabilitiesProbe() {
  let cached: ReturnType<typeof webviewAudioCapabilities> | undefined;
  return () => cached ??= webviewAudioCapabilities();
}
