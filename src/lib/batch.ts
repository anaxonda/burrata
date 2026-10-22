import { isLinkedAudioSearchCandidate } from "./preview-source";
import type { Candidate, Track } from "./types";

export interface BatchSourceResult {
  candidate?: Candidate;
  failures: string[];
}

export interface BatchMatchAssessment {
  action: "inspect" | "accept" | "enrich" | "reject";
  relationship: "exact" | "probable" | "alternative" | "unresolved";
  proofStrength?: "metadataOnly" | "recordingIdentifier" | "audioFingerprint";
  dimensions?: Partial<Record<"title" | "artists" | "guests" | "version" | "runtime" | "isrc" | "work", "agree" | "conflict" | "unknown">>;
  evidence?: {
    identifierConflict?: boolean;
    conflictingVersionObservations?: boolean;
    parsingIncomplete?: boolean;
    unresolvedAnnotations?: unknown[];
  };
}

/**
 * A saved audio-search row is a user's explicit source choice. When its exact
 * linked source still matches title and runtime but artist identity is absent,
 * retain that source as an unverified source identity instead of substituting
 * another provider's result. Contradictions remain a hard stop.
 */
export function isExplicitUnresolvedAudioSearchSource(
  track: Track,
  candidate: Candidate,
  assessment: BatchMatchAssessment,
): boolean {
  const provenance = track.provenance && typeof track.provenance === "object"
    ? track.provenance as Record<string, unknown> : {};
  if (provenance.sourceKind !== "audioSearch" || track.artist.trim() || assessment.action !== "enrich" || assessment.relationship !== "unresolved") return false;
  if (!isLinkedAudioSearchCandidate(track, candidate) || candidate.provider !== track.provider || candidate.id !== track.id) return false;
  if (candidate.id !== `${candidate.provider}:${track.providerId}` || candidate.url !== track.sourceUrl) return false;
  if (candidate.permission === "restricted") return false;
  if (assessment.dimensions?.title !== "agree" || assessment.dimensions?.runtime !== "agree" ||
      assessment.dimensions?.artists !== "unknown") return false;
  if (Object.values(assessment.dimensions).some((value) => value === "conflict")) return false;
  if (assessment.evidence?.identifierConflict || assessment.evidence?.conflictingVersionObservations ||
      assessment.evidence?.parsingIncomplete || (assessment.evidence?.unresolvedAnnotations?.length ?? 0) > 0) return false;
  return true;
}

/** Resolve one track through an ordered set of sources without opening a chooser. */
export async function resolveBatchSource(
  providers: string[],
  linked: Candidate | undefined,
  find: (provider: string) => Promise<Candidate[]>,
  select: (provider: string, candidates: Candidate[]) => Candidate[],
  inspect: (candidate: Candidate) => Promise<Candidate>,
  assess: (candidate: Candidate) => Promise<BatchMatchAssessment>,
  cancelled: () => boolean,
  verifyPeer: (candidate: Candidate) => boolean,
  rankAssessment?: (candidate: Candidate) => Promise<BatchMatchAssessment>,
  acceptExplicitLinked?: (candidate: Candidate, assessment: BatchMatchAssessment) => boolean,
): Promise<BatchSourceResult> {
  const failures: string[] = [];
  const verify = async (candidate: Candidate, isLinked = false): Promise<Candidate | undefined> => {
    if (cancelled() || candidate.permission === "restricted") return undefined;
    try {
      if (candidate.provider === "slskd" && !verifyPeer(candidate)) return undefined;
      const result = candidate.provider === "slskd" ? candidate : await inspect(candidate);
      if (result.provider !== candidate.provider || result.id !== candidate.id || result.url !== candidate.url) {
        failures.push(`${candidate.provider}: inspection changed the selected source identity`);
        return undefined;
      }
      if (cancelled() || result.permission === "restricted") return undefined;
      const assessment = await assess(result);
      if (cancelled()) return undefined;
      if (assessment.action === "accept" && assessment.relationship !== "unresolved") return result;
      return isLinked && acceptExplicitLinked?.(result, assessment) ? result : undefined;
    } catch (error) {
      failures.push(`${candidate.provider}: ${String(error)}`);
      return undefined;
    }
  };

  if (linked && providers.includes(linked.provider)) {
    const verified = await verify(linked, true);
    if (verified) return { candidate: verified, failures };
    if (!cancelled()) failures.push(`${linked.provider}: linked source was not accepted by recording match policy`);
  }

  for (const provider of providers) {
    if (cancelled()) break;
    let candidates: Candidate[];
    try {
      candidates = await find(provider);
    } catch (error) {
      failures.push(`${provider}: ${String(error)}`);
      continue;
    }
    if (cancelled()) break;
    // Legacy source scores are only a fallback. Ask the shared recording policy
    // to prioritize candidates before applying the bounded inspection window;
    // an unavailable/uncertain pre-assessment must never remove a candidate.
    const selected = select(provider, candidates);
    const ranked = await Promise.all(selected.map(async (candidate, index) => {
      if (cancelled() || !rankAssessment) return { candidate, index, error: false, relationship: 0, action: 0, proof: 0, unknowns: 0 };
      try {
        const assessment = await rankAssessment(candidate);
        const relationship = ({ exact: 3, probable: 2, alternative: 1, unresolved: 0 } as const)[assessment.relationship];
        const action = ({ accept: 3, inspect: 2, enrich: 1, reject: 0 } as const)[assessment.action];
        const proof = ({ audioFingerprint: 2, recordingIdentifier: 1, metadataOnly: 0 } as const)[assessment.proofStrength ?? "metadataOnly"];
        const unknowns = Object.values(assessment.dimensions ?? {}).filter(value => value === "unknown").length;
        return { candidate, index, error: false, relationship, action, proof, unknowns };
      } catch {
        return { candidate, index, error: true, relationship: 0, action: 0, proof: 0, unknowns: 7 };
      }
    }));
    if (cancelled()) break;
    const ordered = ranked.sort((a, b) => b.relationship - a.relationship || b.action - a.action || b.proof - a.proof ||
      a.unknowns - b.unknowns || Number(a.error) - Number(b.error) || b.candidate.score - a.candidate.score || a.index - b.index)
      .slice(0, 3).map(({ candidate }) => candidate);
    if (!ordered.length) {
      failures.push(`${provider}: no suitable source found`);
      continue;
    }
    for (const candidate of ordered) {
      const verified = await verify(candidate);
      if (verified) return { candidate: verified, failures };
      if (!cancelled()) failures.push(`${provider}: ${candidate.title || "candidate"} was not accepted by recording match policy`);
    }
  }
  return { failures };
}

/** Batch acquisition requires inspected identity evidence; uncertain candidates stay for review. */
export async function verifiedBatchSource(
  candidates: Candidate[],
  inspect: (candidate: Candidate) => Promise<Candidate>,
  cancelled: () => boolean,
): Promise<Candidate | undefined> {
  for (const candidate of candidates.slice(0, 3)) {
    if (cancelled()) return undefined;
    if (candidate.permission === "restricted") continue;
    try {
      const result = await inspect(candidate);
      if (cancelled()) return undefined;
      if (result.confidence === "high" && result.permission !== "restricted")
        return result;
    } catch {
      /* A broken source does not prevent inspecting another candidate. */
    }
  }
  return undefined;
}
