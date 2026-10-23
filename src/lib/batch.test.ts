import { describe, expect, it, vi } from "vitest";
import { isExplicitSoulseekFolderSource, isExplicitUnresolvedAudioSearchSource, resolveBatchSource, verifiedBatchSource } from "./batch";
import type { Candidate, Track } from "./types";
const candidate = (id: string, confidence = "high"): Candidate => ({
  id,
  provider: "youtube",
  url: `https://youtube.com/watch?v=${id}`,
  title: id,
  formats: [],
  permission: "unknown",
  confidence,
  score: 0,
  reasons: [],
  official: false,
});
const accepted = async () => ({ action: "accept" as const, relationship: "exact" as const });
const rejected = async () => ({ action: "inspect" as const, relationship: "probable" as const });
describe("selected batch acquisition", () => {
  it("falls through a preferred-source miss to the configured source and keeps high-confidence verification", async () => {
    const find = vi.fn(async (provider: string) => provider === "youtube" ? [candidate("exact-match")] : []);
    const result = await resolveBatchSource(
      ["soundcloud", "youtube"], undefined, find,
      (provider, rows) => rows.filter((row) => row.provider === provider),
      async (row) => row, accepted, () => false, () => false,
    );
    expect(find).toHaveBeenNthCalledWith(1, "soundcloud");
    expect(find).toHaveBeenNthCalledWith(2, "youtube");
    expect(result.candidate?.id).toBe("exact-match");
  });

  it("reuses a linked source before searching providers", async () => {
    const find = vi.fn(async () => []);
    const linked = candidate("linked");
    const result = await resolveBatchSource(
      ["youtube", "soundcloud"], linked, find,
      (_provider, rows) => rows, async (row) => row, accepted, () => false, () => false,
    );
    expect(result.candidate?.id).toBe("linked");
    expect(find).not.toHaveBeenCalled();
  });

  it("retains an exactly linked saved audio-search source when only artist identity is missing", async () => {
    const track: Track = {
      id: "youtube:rumba-id", provider: "youtube", providerId: "rumba-id",
      sourceUrl: "https://www.youtube.com/watch?v=rumba-id", artist: "", title: "La Rumba",
      durationSeconds: 216, favorite: false, provenance: { sourceKind: "audioSearch" },
    };
    const linked: Candidate = {
      ...candidate("youtube:rumba-id", "low"),
      url: track.sourceUrl, title: "La Rumba", durationSeconds: 216,
    };
    const assessment = {
      action: "enrich" as const, relationship: "unresolved" as const,
      proofStrength: "metadataOnly" as const,
      dimensions: { title: "agree" as const, runtime: "agree" as const, artists: "unknown" as const,
        guests: "unknown" as const, version: "unknown" as const, isrc: "unknown" as const, work: "unknown" as const },
      evidence: { identifierConflict: false, conflictingVersionObservations: false, parsingIncomplete: false,
        unresolvedAnnotations: [] },
    };
    expect(isExplicitUnresolvedAudioSearchSource(track, linked, assessment)).toBe(true);
    const find = vi.fn(async () => []);
    const result = await resolveBatchSource(
      ["monochrome", "youtube", "soundcloud", "slskd"], linked, find,
      (_provider, rows) => rows, async row => row, async () => assessment, () => false,
      () => false, undefined,
      (row, decision) => isExplicitUnresolvedAudioSearchSource(track, row, decision),
    );
    expect(result.candidate?.id).toBe(linked.id);
    expect(find).not.toHaveBeenCalled();
  });

  it("retains only the exact explicitly selected Soulseek folder locator when identity is unverified", async () => {
    const linked: Candidate = {
      ...candidate("slskd:folder-file", "low"),
      provider: "slskd", url: "", title: "Totem - Lamomali, Yamê", durationSeconds: 179.5,
      source: { kind: "soulseek", username: "folder-peer", filename: "Music/Album/57 - Totem.flac", size_bytes: 4096 },
    };
    const track: Track = {
      id: linked.id, provider: "slskd", providerId: "folder-file", sourceUrl: "",
      artist: "", title: linked.title, durationSeconds: 179.5, favorite: false,
      provenance: { sourceKind: "soulseekFolder", audioCandidate: linked },
    };
    const assessment = {
      action: "enrich" as const, relationship: "unresolved" as const,
      proofStrength: "metadataOnly" as const,
      dimensions: { title: "agree" as const, runtime: "agree" as const, artists: "unknown" as const,
        guests: "unknown" as const, version: "unknown" as const, isrc: "unknown" as const, work: "unknown" as const },
      evidence: { identifierConflict: false, conflictingVersionObservations: false,
        parsingIncomplete: false, unresolvedAnnotations: [] },
    };
    expect(isExplicitSoulseekFolderSource(track, linked, assessment)).toBe(true);

    const find = vi.fn(async () => []);
    const result = await resolveBatchSource(
      ["slskd"], linked, find, (_provider, rows) => rows,
      async row => row, async () => assessment, () => false,
      row => row.provider === "slskd" && Boolean(row.source?.filename), undefined,
      (row, decision) => isExplicitSoulseekFolderSource(track, row, decision),
    );
    expect(result.candidate?.id).toBe(linked.id);
    expect(find).not.toHaveBeenCalled();

    expect(isExplicitSoulseekFolderSource(
      { ...track, provenance: { sourceKind: "catalog", audioCandidate: linked } }, linked, assessment,
    )).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, { ...linked, id: "slskd:other" }, assessment)).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, { ...linked, provider: "youtube" }, assessment)).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, {
      ...linked, source: { ...linked.source!, filename: "Music/Other/57 - Totem.flac" },
    }, assessment)).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, {
      ...linked, source: { ...linked.source!, username: "other-peer" },
    }, assessment)).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, {
      ...linked, source: { ...linked.source!, size_bytes: linked.source!.size_bytes + 1 },
    }, assessment)).toBe(false);
    expect(isExplicitSoulseekFolderSource({
      ...track,
      provenance: { sourceKind: "soulseekFolder", audioCandidate: {
        ...linked, source: { ...linked.source!, filename: "Music/Other/57 - Totem.flac" },
      } },
    }, linked, assessment)).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, {
      ...linked, permission: "restricted",
    }, assessment)).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, linked, {
      ...assessment,
      dimensions: { ...assessment.dimensions, artists: "conflict" },
    })).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, linked, {
      ...assessment,
      dimensions: { ...assessment.dimensions, version: "conflict" },
    })).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, linked, {
      ...assessment,
      evidence: { ...assessment.evidence, parsingIncomplete: true },
    })).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, linked, {
      ...assessment,
      evidence: { ...assessment.evidence, identifierConflict: true },
    })).toBe(false);
    expect(isExplicitSoulseekFolderSource(track, linked, {
      ...assessment,
      evidence: { ...assessment.evidence, unresolvedAnnotations: [{ value: "Live" }] },
    })).toBe(false);
  });

  it("does not use the linked-source exception for mismatches, conflicts, non-audioSearch rows, or restricted sources", () => {
    const track: Track = {
      id: "youtube:rumba-id", provider: "youtube", providerId: "rumba-id",
      sourceUrl: "https://www.youtube.com/watch?v=rumba-id", artist: "", title: "La Rumba",
      durationSeconds: 216, favorite: false, provenance: { sourceKind: "audioSearch" },
    };
    const linked: Candidate = { ...candidate("youtube:rumba-id", "low"), url: track.sourceUrl,
      title: track.title, durationSeconds: 216 };
    const assessment = {
      action: "enrich" as const, relationship: "unresolved" as const,
      dimensions: { title: "agree" as const, runtime: "agree" as const, artists: "unknown" as const,
        guests: "unknown" as const, version: "unknown" as const, isrc: "unknown" as const, work: "unknown" as const },
      evidence: { identifierConflict: false, conflictingVersionObservations: false, parsingIncomplete: false },
    };
    expect(isExplicitUnresolvedAudioSearchSource(track, { ...linked, url: "https://www.youtube.com/watch?v=other" }, assessment)).toBe(false);
    expect(isExplicitUnresolvedAudioSearchSource({ ...track, provenance: { sourceKind: "catalog" } }, linked, assessment)).toBe(false);
    expect(isExplicitUnresolvedAudioSearchSource({ ...track, provenance: { sourceKind: "youtubeRadio" } }, linked, assessment)).toBe(false);
    expect(isExplicitUnresolvedAudioSearchSource(track, { ...linked, permission: "restricted" }, assessment)).toBe(false);
    expect(isExplicitUnresolvedAudioSearchSource(track, linked, { ...assessment, dimensions: { ...assessment.dimensions, artists: "conflict" } })).toBe(false);
    expect(isExplicitUnresolvedAudioSearchSource(track, linked, { ...assessment, dimensions: { ...assessment.dimensions, version: "conflict" } })).toBe(false);
    expect(isExplicitUnresolvedAudioSearchSource(track, linked, { ...assessment, evidence: { ...assessment.evidence, identifierConflict: true } })).toBe(false);
  });

  it("continues after a source failure and rejects an unaccepted version", async () => {
    const find = vi.fn(async (provider: string) => {
      if (provider === "youtube") throw new Error("offline");
      return [{ ...candidate("wrong-version"), provider: "soundcloud" }];
    });
    const result = await resolveBatchSource(
      ["youtube", "soundcloud"], undefined, find,
      (provider, rows) => rows.filter((row) => row.provider === provider),
      async (row) => ({ ...row, confidence: "review" }), rejected, () => false, () => false,
    );
    expect(result.candidate).toBeUndefined();
    expect(result.failures.some((message) => message.includes("youtube:") && message.includes("offline"))).toBe(true);
    expect(result.failures.some((message) => message.includes("not accepted by recording match policy"))).toBe(true);
  });

  it("rejects inspection that changes the selected provider source", async () => {
    const original = candidate("original");
    const result = await resolveBatchSource(
      ["youtube"], undefined, async () => [original],
      (_provider, rows) => rows,
      async (row) => ({ ...row, id: "substituted", confidence: "high" }),
      accepted,
      () => false, () => false,
    );
    expect(result.candidate).toBeUndefined();
    expect(result.failures.some((message) => message.includes("changed the selected source identity"))).toBe(true);
  });

  it("does not trust a search recommendation that inspection downgrades", async () => {
    expect(
      await verifiedBatchSource(
        [candidate("radio-edit")],
        async (c) => ({ ...c, confidence: "review" }),
        () => false,
      ),
    ).toBeUndefined();
  });
  it("accepts a probable exact-source decision from the shared policy even when legacy confidence is review", async () => {
    const result = await resolveBatchSource(
      ["monochrome"], undefined, async () => [candidate("isrc-exact", "review")],
      (_provider, rows) => rows,
      async (row) => row,
      async () => ({ action: "accept", relationship: "probable" }),
      () => false, () => false,
    );
    expect(result.candidate?.id).toBe("isrc-exact");
  });
  it("requires shared recording acceptance for a verified peer file too", async () => {
    const peer = {...candidate("peer"),provider:"slskd"};
    const inspect = vi.fn(async(row:Candidate)=>row);
    const result=await resolveBatchSource(["slskd"],peer,async()=>[],(_provider,rows)=>rows,inspect,rejected,()=>false,()=>true);
    expect(result.candidate).toBeUndefined();
    expect(inspect).not.toHaveBeenCalled();
  });
  it("honors an accepted Flexible alternative while rejecting unresolved recording identity", async () => {
    const run = (relationship: "alternative" | "unresolved") => resolveBatchSource(
      ["youtube"],undefined,async()=>[candidate(relationship)],(_provider,rows)=>rows,
      async row=>row,async()=>({action:"accept",relationship}),()=>false,()=>false,
    );
    expect((await run("alternative")).candidate?.id).toBe("alternative");
    expect((await run("unresolved")).candidate).toBeUndefined();
  });
  it("ranks a low legacy score accepted alternative ahead of higher scored unrelated results", async () => {
    const flexible = { ...candidate("flexible", "low"), score: 0.49 };
    const unrelated = ["unrelated-1", "unrelated-2", "unrelated-3"].map((id, i) => ({
      ...candidate(id), score: 0.99 - i * 0.01,
    }));
    const inspected: string[] = [];
    const assessments: Record<string, { action: "accept" | "reject"; relationship: "alternative" | "unresolved" }> = {
      flexible: { action: "accept", relationship: "alternative" },
      ...Object.fromEntries(unrelated.map(row => [row.id, { action: "reject" as const, relationship: "unresolved" as const }])),
    };
    const result = await resolveBatchSource(
      ["youtube"], undefined, async () => [ ...unrelated, flexible ], (_provider, rows) => rows,
      async row => { inspected.push(row.id); return row; },
      async row => assessments[row.id], () => false, () => false,
      async row => assessments[row.id],
    );
    expect(inspected).toEqual(["flexible"]);
    expect(result.candidate?.id).toBe("flexible");
  });
  it("prioritizes exact relationship over alternative acceptance before score", async () => {
    const exactInspect = { ...candidate("exact-inspect"), score: 0.1 };
    const alternativeAccept = { ...candidate("alternative-accept"), score: 0.99 };
    const inspected: string[] = [];
    const result = await resolveBatchSource(
      ["youtube"], undefined, async () => [alternativeAccept, exactInspect], (_provider, rows) => rows,
      async row => { inspected.push(row.id); return row; }, accepted, () => false, () => false,
      async row => row.id === exactInspect.id
        ? { action: "inspect", relationship: "exact" }
        : { action: "accept", relationship: "alternative" },
    );
    expect(inspected[0]).toBe("exact-inspect");
    expect(result.candidate?.id).toBe("exact-inspect");
  });
  it("uses stronger identifier proof before legacy score and inspects no more than three candidates", async () => {
    const rows = [
      { ...candidate("metadata"), score: 0.99 },
      { ...candidate("identifier"), score: 0.1 },
      { ...candidate("metadata-2"), score: 0.2 },
      { ...candidate("below-window"), score: 0.3 },
    ];
    const inspected: string[] = [];
    const result = await resolveBatchSource(
      ["youtube"], undefined, async () => rows, (_provider, values) => values,
      async row => { inspected.push(row.id); return row; }, accepted, () => false, () => false,
      async row => ({
        action: "accept", relationship: "probable",
        proofStrength: row.id === "identifier" ? "recordingIdentifier" : "metadataOnly",
      }),
    );
    expect(inspected).toEqual(["identifier"]);
    expect(result.candidate?.id).toBe("identifier");
  });
  it("does not filter a candidate when the pre-inspection ranking assessment fails", async () => {
    const row = { ...candidate("unknown"), score: 0.1 };
    const result = await resolveBatchSource(
      ["youtube"], undefined, async () => [row], (_provider, rows) => rows,
      async candidate => candidate, accepted, () => false, () => false,
      async () => { throw new Error("assessment unavailable"); },
    );
    expect(result.candidate?.id).toBe("unknown");
  });
  it("continues past a failed source and uses verified identity", async () => {
    const inspect = vi.fn(async (c: Candidate) => {
      if (c.id === "offline") throw new Error("offline");
      return c;
    });
    expect(
      (
        await verifiedBatchSource(
          [candidate("offline"), candidate("extended")],
          inspect,
          () => false,
        )
      )?.id,
    ).toBe("extended");
  });
  it("stops after cancellation even when an in-flight inspection succeeds", async () => {
    let cancelled = false;
    const inspect = vi.fn(async (c: Candidate) => {
      cancelled = true;
      return c;
    });
    expect(
      await verifiedBatchSource(
        [candidate("first"), candidate("second")],
        inspect,
        () => cancelled,
      ),
    ).toBeUndefined();
    expect(inspect).toHaveBeenCalledOnce();
  });
});
