import { describe, expect, it } from "vitest";
import { claimAutomaticStreamMediaAnalysis, streamAudioVariantIdentity, streamAudioVariantSha256, fullStreamAnalysisOutcome, isCompleteStreamMediaWaveform, streamMediaResourceKey, streamMediaTrackContext, verifiedStreamWaveform, type StreamMediaAnalysis, type StreamMediaResource } from "./stream-media-analysis";
import type { Track } from "./types";

const resource: StreamMediaResource = { trackId: "recording-1", provider: "youtube", sourceId: "video-1", durationSeconds: 30 };
const completeAnalysis = async (): Promise<StreamMediaAnalysis> => {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("youtube\nvideo-1"));
  const sourceIdentitySha256 = [...new Uint8Array(digest)].map(value => value.toString(16).padStart(2, "0")).join("");
  return {
    schemaVersion: 1, analyzerVersion: "stream-analysis-v1",
    resource: { kind: "stream", recordingId: resource.trackId, provider: resource.provider, sourceIdentitySha256 },
    durationSeconds: 30, scope: "wholeTrack", coverage: 1, measuredAt: "2026-10-25T00:00:00Z",
    bpm: { bpm: 120, confidence: 0.8 }, key: null,
    waveform: { analyzerVersion: "stream-analysis-v1", binCount: 512, durationSeconds: 30,
      bins: Array.from({ length: 512 }, (_, index) => ({ min: -index / 512, max: index / 512 })),
      scope: "wholeTrack", complete: true, measuredAt: "2026-10-25T00:00:00Z" },
  };
};

describe("verified streamed media analysis", () => {
  it("binds cached waveforms to the encoding and length without signed URLs or rounded bitrate", async () => {
    const variant = { formatId: "140", codec: "mp4a.40.2", mimeType: "audio/mp4", containsVideo: false, expectedContentLength: 123456 };
    const digest = await streamAudioVariantSha256(variant);
    expect(digest).toBe("d39cf402ad2591ff335487243ebe454a6972fc1ef87a43e72d1dad74392e60be");
    expect(streamAudioVariantIdentity({ ...variant, expectedContentLength: 0 })).toBeNull();
    expect(streamAudioVariantIdentity({ ...variant, expectedContentLength: Number.MAX_SAFE_INTEGER + 1 })).toBeNull();
    expect(await streamAudioVariantSha256({ ...variant, formatId: " 140 ", codec: "MP4A.40.2", bitrateKbps: 129 })).toBe(digest);
    expect(await streamAudioVariantSha256({ ...variant, expectedContentLength: 123457 })).not.toBe(digest);
    expect(await streamAudioVariantSha256({ ...variant, formatId: "18", containsVideo: true })).not.toBe(digest);
    expect(streamAudioVariantIdentity({ codec: "flac" })).toBeNull();
    expect(streamAudioVariantIdentity({ formatId: "K", codec: "flac" })).toBeNull();
    expect(streamAudioVariantIdentity({ formatId: "https://private.test/?token=secret", codec: "flac" })).toBeNull();
    const analysis = await completeAnalysis();
    if (analysis.resource.kind !== "stream") throw Error("Expected stream fixture");
    analysis.resource.audioVariantSha256 = digest;
    const encodedResource = { ...resource, audioVariant: variant };
    expect(await verifiedStreamWaveform(analysis, encodedResource, 17)).not.toBeNull();
    expect(await verifiedStreamWaveform(analysis, { ...encodedResource, audioVariant: { ...variant, formatId: "18" } }, 17)).toBeNull();
    expect(await verifiedStreamWaveform(analysis, resource, 17)).toBeNull();
    expect(streamMediaResourceKey(encodedResource)).not.toBe(streamMediaResourceKey({ ...encodedResource, audioVariant: { ...variant, expectedContentLength: 999 } }));
    expect(streamMediaResourceKey({ ...resource, sessionGeneration: 1 })).not.toBe(streamMediaResourceKey({ ...resource, sessionGeneration: 2 }));
  });

  it("starts automatic analysis only when opted in and at most once per exact resource", () => {
    const attempted = new Set<string>();
    expect(claimAutomaticStreamMediaAnalysis(false, true, "source-a", attempted)).toBe(false);
    expect(claimAutomaticStreamMediaAnalysis(true, false, "source-a", attempted)).toBe(false);
    expect(claimAutomaticStreamMediaAnalysis(true, true, null, attempted)).toBe(false);
    expect(claimAutomaticStreamMediaAnalysis(true, true, "source-a", attempted)).toBe(true);
    expect(claimAutomaticStreamMediaAnalysis(true, true, "source-a", attempted)).toBe(false);
    expect(claimAutomaticStreamMediaAnalysis(true, true, "source-b", attempted)).toBe(true);
  });

  it("projects only bounded resolver metadata and omits private playback fields", () => {
    const context = streamMediaTrackContext({
      id: "recording-1", provider: "playlist", providerId: "catalog-1", title: "Title", artist: "Artist",
      sourceUrl: "https://private.example/signed?token=secret", localPath: "/private/music/file.flac",
      artworkUrl: "https://private.example/art?token=secret", previewUrl: "https://private.example/audio?token=secret",
      provenance: { audioCandidate: { url: "https://private.example/audio?token=secret" }, authorization: "secret" },
      bpm: 123, key: "C", durationSeconds: 30,
    } as Track);
    expect(context).toEqual(expect.objectContaining({ id: "recording-1", title: "Title", artist: "Artist", providerId: "catalog-1" }));
    expect(context.hasPlaybackRange).toBe(false);
    expect(context).not.toHaveProperty("sourceUrl");
    expect(context).not.toHaveProperty("localPath");
    expect(context).not.toHaveProperty("provenance");
    expect(context).not.toHaveProperty("bpm");
    expect(JSON.stringify(context)).not.toContain("secret");
    const ranged = streamMediaTrackContext({ ...({ id: "recording-1", provider: "youtube", providerId: "video-1", title: "Title", artist: "Artist", playbackRange: { parentTrackId: "parent", chapterId: "chapter", sourceUrl: "https://private.example/set", startTime: 10, endTime: 20 } } as Track) });
    expect(ranged.hasPlaybackRange).toBe(true);
  });

  it("includes only canonical public provider pages tied to the active source identity", () => {
    const base = { id: "recording-1", provider: "youtube", providerId: "video-1", title: "Title", artist: "Artist", sourceUrl: "https://www.youtube.com/watch?v=video-1" } as Track;
    expect(streamMediaTrackContext(base, "youtube", "video-1").sourcePageUrl).toBe("https://www.youtube.com/watch?v=video-1");
    expect(streamMediaTrackContext({ ...base, sourceUrl: "https://www.youtube.com/watch?v=other" }, "youtube", "video-1").sourcePageUrl).toBeUndefined();
    expect(streamMediaTrackContext({ ...base, sourceUrl: "https://www.youtube.com/watch?v=video-1&token=private" }, "youtube", "video-1").sourcePageUrl).toBeUndefined();
    expect(streamMediaTrackContext({ ...base, provider: "bandcamp", sourceUrl: "https://artist.bandcamp.com/track/song" }, "bandcamp", "bandcamp-id").sourcePageUrl)
      .toBe("https://artist.bandcamp.com/track/song");
    expect(streamMediaTrackContext({ ...base, provider: "bandcamp", sourceUrl: "https://artist.bandcamp.com/track/song?token=private" }, "bandcamp", "bandcamp-id").sourcePageUrl).toBeUndefined();
  });

  it("keeps restored artistless YouTube context stable when playback hydrates provider fields", () => {
    const videoId = "lSmAz2rIBnE";
    const sourceId = `youtube:${videoId}`;
    const restored = {
      id: sourceId, provider: "", providerId: "", title: "Tengo tengo", artist: "",
      sourceUrl: `https://www.youtube.com/watch?v=${videoId}`, durationSeconds: 155,
    } as Track;
    const hydrated = { ...restored, provider: "youtube", providerId: videoId } as Track;

    const restoredContext = streamMediaTrackContext(restored, "youtube", sourceId);
    const hydratedContext = streamMediaTrackContext(hydrated, "youtube", sourceId);
    expect(restoredContext).toEqual(hydratedContext);
    expect(restoredContext).toEqual(expect.objectContaining({
      id: sourceId, provider: "youtube", providerId: videoId,
      sourcePageUrl: `https://www.youtube.com/watch?v=${videoId}`,
      artist: "", title: "Tengo tengo",
    }));

    for (const sourceUrl of [
      "https://www.youtube.com/watch?v=AbcDefGhi_1",
      `https://evil.example/watch?v=${videoId}`,
      `https://user:pass@www.youtube.com/watch?v=${videoId}`,
      `https://www.youtube.com/watch?v=${videoId}&feature=share`,
      `https://www.youtube.com:8443/watch?v=${videoId}`,
    ]) {
      const rejected = streamMediaTrackContext({ ...restored, sourceUrl }, "youtube", sourceId);
      expect(rejected.provider).toBe("unknown");
      expect(rejected.sourcePageUrl).toBeUndefined();
    }
    expect(streamMediaTrackContext({ ...restored, id: "catalog-recording" }, "youtube", sourceId).provider).toBe("unknown");
    expect(streamMediaTrackContext({ ...restored, provider: "bandcamp" }, "youtube", sourceId).provider).toBe("bandcamp");
    expect(streamMediaTrackContext({ ...restored, providerId: "AbcDefGhi_1" }, "youtube", sourceId).sourcePageUrl).toBeUndefined();
  });

  it("binds full waveform snapshots to the exact provider resource and duration", async () => {
    const analysis = await completeAnalysis();
    expect(isCompleteStreamMediaWaveform(analysis)).toBe(true);
    expect(streamMediaResourceKey(resource)).toContain("youtube");
    const waveform = await verifiedStreamWaveform(analysis, resource, 17);
    expect(waveform?.complete).toBe(true);
    expect(waveform?.source).toBe("stream-analysis");
    expect(waveform?.resource).toEqual({ provider: "youtube", sourceId: "video-1", sourceIdentitySha256: analysis.resource.kind === "stream" ? analysis.resource.sourceIdentitySha256 : "", bindingKey: streamMediaResourceKey(resource) });
    expect(await verifiedStreamWaveform(analysis, { ...resource, sourceId: "different-video" }, 17)).toBeNull();
    expect(await verifiedStreamWaveform(analysis, { ...resource, provider: "soundcloud" }, 17)).toBeNull();
    expect(await verifiedStreamWaveform(analysis, { ...resource, trackId: "different-track" }, 17)).toBeNull();
    expect(await verifiedStreamWaveform(analysis, { ...resource, durationSeconds: 31 }, 17)).toBeNull();
  });

  it("does not report successful full analysis when a finished batch rejected the recording", async () => {
    const analysis = await completeAnalysis();
    expect(fullStreamAnalysisOutcome("completed", { status: "failed", analysis: null })).toBe("failed");
    expect(fullStreamAnalysisOutcome("completed", undefined)).toBe("failed");
    expect(fullStreamAnalysisOutcome("completed", { status: "completed", analysis: { ...analysis, waveform: null } })).toBe("failed");
    expect(fullStreamAnalysisOutcome("completed", { status: "completed", analysis })).toBe("completed");
    expect(fullStreamAnalysisOutcome("running", undefined)).toBe("running");
    expect(fullStreamAnalysisOutcome("cancelled", undefined)).toBe("cancelled");
  });

  it("keeps partial, invalid, or absent waveform data unavailable", async () => {
    const analysis = await completeAnalysis();
    const partial = { ...analysis, waveform: { ...analysis.waveform!, complete: false } };
    expect(isCompleteStreamMediaWaveform(partial)).toBe(false);
    expect(await verifiedStreamWaveform(partial, resource, 1)).toBeNull();
    expect(await verifiedStreamWaveform({ ...analysis, waveform: null }, resource, 1)).toBeNull();
    const malformed = { ...analysis, waveform: { ...analysis.waveform!, bins: [null, ...analysis.waveform!.bins.slice(1)] } };
    expect(await verifiedStreamWaveform(malformed, resource, 1)).toBeNull();
  });
});
