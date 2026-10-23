import { describe, expect, it } from "vitest";
import { sanitizeWebviewSourceFormatDiagnostics } from "./webview-source-format-diagnostics";

describe("WebView source format diagnostics", () => {
  it("retains bounded allowlisted format data and omits URL-shaped fields", () => {
    const raw = {
      codec: "mp4a.40.2", mimeType: "audio/mp4", formatId: "140", hasVideo: false,
      expectedContentLength: 123456, url: "https://private.invalid/?token=secret", headers: { authorization: "secret" },
    };
    const safe = sanitizeWebviewSourceFormatDiagnostics(raw, "direct");
    expect(safe).toEqual({ codec: "aac", mimeType: "audio/mp4", formatId: "140", hasVideo: false, expectedContentLength: 123456, streamKind: "direct" });
    expect(JSON.stringify(safe)).not.toContain("private.invalid");
    expect(JSON.stringify(safe)).not.toContain("secret");
  });

  it("normalizes absent and unsupported metadata to null without breaking inspection", () => {
    expect(sanitizeWebviewSourceFormatDiagnostics({
      codec: "opus\\nhttps://private.invalid", mimeType: "audio/mp4; token=secret", formatId: "bad/id",
      hasVideo: "unknown", expectedContentLength: Number.MAX_SAFE_INTEGER,
    }, "unknown")).toEqual({ codec: null, mimeType: null, formatId: null, hasVideo: null, expectedContentLength: null, streamKind: "unavailable" });
    expect(sanitizeWebviewSourceFormatDiagnostics({}, null)).toEqual({ codec: null, mimeType: null, formatId: null, hasVideo: null, expectedContentLength: null, streamKind: "unavailable" });
  });
});
