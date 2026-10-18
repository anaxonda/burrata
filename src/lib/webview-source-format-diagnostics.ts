export type WebviewSourceFormatDiagnostics = {
  codec: string | null;
  mimeType: string | null;
  formatId: string | null;
  hasVideo: boolean | null;
  expectedContentLength: number | null;
  streamKind: "direct" | "hls" | "unavailable";
};

const safeToken = (value: unknown, pattern: RegExp, maxLength: number): string | null =>
  typeof value === "string" && value.length > 0 && value.length <= maxLength && pattern.test(value) ? value : null;

/** Allowlist diagnostic format metadata; never accepts or returns URL/header properties. */
export function sanitizeWebviewSourceFormatDiagnostics(
  value: { codec?: unknown; mimeType?: unknown; formatId?: unknown; hasVideo?: unknown; expectedContentLength?: unknown },
  streamKind: unknown,
): WebviewSourceFormatDiagnostics {
  const length = value.expectedContentLength;
  return {
    codec: safeToken(value.codec, /^[A-Za-z0-9][A-Za-z0-9._+-]*$/, 64),
    mimeType: safeToken(value.mimeType, /^[A-Za-z0-9][A-Za-z0-9.+-]*\/[A-Za-z0-9][A-Za-z0-9.+-]*$/, 96),
    formatId: safeToken(value.formatId, /^[A-Za-z0-9][A-Za-z0-9.+-]*$/, 40),
    hasVideo: typeof value.hasVideo === "boolean" ? value.hasVideo : null,
    expectedContentLength: Number.isSafeInteger(length) && Number(length) >= 0 && Number(length) <= 17_179_869_184 ? Number(length) : null,
    streamKind: streamKind === "direct" || streamKind === "hls" ? streamKind : "unavailable",
  };
}
