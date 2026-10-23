/** Fixed player artwork diagnostics. Never returns URLs, paths, text, or metadata. */
export async function inspectArtworkImages(root: Document, artworkUrl: string, blacklisted: boolean) {
  const parsedUrl = (url: string): URL | null => { try { return url ? new URL(url) : null; } catch { return null; } };
  const scheme = (url: string): string => {
    const protocol = parsedUrl(url)?.protocol.slice(0, -1);
    return protocol && ["asset", "http", "https", "data", "blob"].includes(protocol) ? protocol : url ? "other" : "none";
  };
  // Only compare the two known Tauri asset URL forms. The decoded path is used
  // locally for equality and is never included in the returned diagnostics.
  const ownedAssetPath = (url: string): string | null => {
    const parsed = parsedUrl(url);
    const knownAuthority = (parsed?.protocol === "asset:" && /^asset:\/\/localhost(?:\/|[?#]|$)/i.test(url))
      || (parsed?.protocol === "http:" && /^http:\/\/asset\.localhost(?:\/|[?#]|$)/i.test(url));
    if (!parsed || !knownAuthority || parsed.username || parsed.password || parsed.port) return null;
    try { const path = decodeURIComponent(parsed.pathname); return path.includes("\0") ? null : path; } catch { return null; }
  };
  const matchesExpected = (url: string): boolean => {
    const actualPath = ownedAssetPath(url), expectedPath = ownedAssetPath(artworkUrl);
    return actualPath !== null && expectedPath !== null && actualPath === expectedPath;
  };
  const images = ([ ["footer", "footer.player .mini-art img"], ["details", ".playing-info .track-hero img"], ["fullscreen", ".now-playing-view .hero-art img"] ] as const).map(([role, selector]) => {
    const image = root.querySelector<HTMLImageElement>(selector);
    const src = image?.src ?? "", currentSrc = image?.currentSrc ?? "";
    const loadState = !image ? "absent" : !image.complete ? "loading" : image.naturalWidth > 0 ? "loaded" : "failed";
    return { role, present: Boolean(image), complete: image?.complete ?? false,
      naturalWidth: image?.naturalWidth ?? 0, naturalHeight: image?.naturalHeight ?? 0,
      urlScheme: scheme(currentSrc || src), srcScheme: scheme(src), currentSrcScheme: scheme(currentSrc),
      srcMatchesExpected: matchesExpected(src), currentSrcMatchesExpected: matchesExpected(currentSrc), loadState };
  });
  let probeStatus = "not_local", probeWidth = 0, probeHeight = 0, cspBlocked = false;
  let probeSrcScheme = "none", probeCurrentSrcScheme = "none", probeSrcMatchesExpected = false, probeCurrentSrcMatchesExpected = false, probeOnError = false;
  // Only retry the currently selected app-owned asset URL. No remote requests.
  if (ownedAssetPath(artworkUrl) !== null) {
    probeStatus = await new Promise<string>((resolve) => {
      const probe = new Image();
      const blocked = (event: SecurityPolicyViolationEvent) => {
        if (event.effectiveDirective === "img-src" && (event.blockedURI === artworkUrl || event.blockedURI === probe.src || ["asset", "asset:", "asset://localhost"].includes(event.blockedURI))) cspBlocked = true;
      };
      const done = (status: string) => {
        clearTimeout(timer); root.removeEventListener("securitypolicyviolation", blocked);
        probeWidth = probe.naturalWidth; probeHeight = probe.naturalHeight;
        probeSrcScheme = scheme(probe.src); probeCurrentSrcScheme = scheme(probe.currentSrc);
        probeSrcMatchesExpected = matchesExpected(probe.src); probeCurrentSrcMatchesExpected = matchesExpected(probe.currentSrc);
        probe.onload = null; probe.onerror = null; resolve(status);
      };
      const timer = setTimeout(() => done("timeout"), 1500);
      root.addEventListener("securitypolicyviolation", blocked);
      probe.onload = () => done("loaded"); probe.onerror = () => { probeOnError = true; done("failed"); };
      // Tauri serves the same asset path regardless of query. Force a fresh
      // request so a cached failure cannot mask the current protocol response.
      const probeUrl = new URL(artworkUrl);
      probeUrl.searchParams.set("burrataArtworkProbe", String(Date.now()));
      probe.src = probeUrl.href;
    });
  }
  return { urlScheme: scheme(artworkUrl), blacklisted, images, probeStatus, probeWidth, probeHeight, cspBlocked,
    probeSrcScheme, probeCurrentSrcScheme, probeSrcMatchesExpected, probeCurrentSrcMatchesExpected, probeOnError };
}
