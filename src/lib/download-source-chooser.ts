import type { Candidate } from "../lib/types";

export type CandidateSort = "match" | "quality" | "size" | "availability" | "duration";
export type CandidateGroup = { provider: string; candidates: Candidate[] };

const providerName = (candidate: Candidate) => candidate.provider.trim() || "Other source";
const formats = (candidate: Candidate) => candidate.formats ?? [];
const advertisedLossless = (candidate: Candidate) => formats(candidate).some((format) => format.lossless);
const bitrate = (candidate: Candidate) => {
  const values = formats(candidate).map((format) => format.bitrate).filter((value): value is number => typeof value === "number" && value > 0);
  return values.length ? Math.max(...values) : candidate.source?.bitrate_kbps ?? null;
};
const size = (candidate: Candidate) => candidate.source?.size_bytes ?? 0;
const availability = (candidate: Candidate) => candidate.permission === "restricted" ? 0 : candidate.source?.username?.trim() ? 1 : candidate.url ? 2 : 0;

function compareMatch(a: Candidate, b: Candidate) {
  const confidence = (value: string) => ({ high: 3, medium: 2, low: 1 }[value.toLowerCase()] ?? 0);
  return confidence(b.confidence) - confidence(a.confidence) || b.score - a.score;
}

function compareQuality(a: Candidate, b: Candidate) {
  // Lossless is a declared source property, not proof that a peer file is authentic.
  // Bitrate only ranks candidates when they report the same codec.
  const losslessDifference = Number(advertisedLossless(b)) - Number(advertisedLossless(a));
  if (losslessDifference) return losslessDifference;
  const codecOf = (candidate: Candidate) => formats(candidate)[0]?.codec?.toLowerCase() || "";
  if (codecOf(a) === codecOf(b) && bitrate(a) != null && bitrate(b) != null) return bitrate(b)! - bitrate(a)!;
  return 0;
}

export function groupDownloadCandidates(
  candidates: Candidate[],
  options: { preferredProvider?: string; providerOrder?: string[]; sort?: CandidateSort; direction?: "asc" | "desc" } = {},
): CandidateGroup[] {
  const { preferredProvider = "", providerOrder = [], sort = "quality", direction = "asc" } = options;
  const buckets = new Map<string, Candidate[]>();
  for (const candidate of candidates) {
    const name = providerName(candidate);
    const bucket = buckets.get(name) ?? [];
    bucket.push(candidate);
    buckets.set(name, bucket);
  }
  const configured = [preferredProvider, ...providerOrder].filter(Boolean);
  const names = [...buckets.keys()].sort((a, b) => {
    const ai = configured.indexOf(a), bi = configured.indexOf(b);
    if (ai >= 0 || bi >= 0) return (ai < 0 ? Infinity : ai) - (bi < 0 ? Infinity : bi) || a.localeCompare(b);
    return a.localeCompare(b);
  });
  return names.map((provider) => {
    const source = buckets.get(provider)!;
    const ordered = source.map((candidate, index) => ({ candidate, index }));
    ordered.sort((a, b) => {
      let result = 0;
      if (sort === "match") result = compareMatch(a.candidate, b.candidate);
      if (sort === "quality") result = compareQuality(a.candidate, b.candidate) || compareMatch(a.candidate, b.candidate);
      if (sort === "size") result = size(b.candidate) - size(a.candidate) || compareMatch(a.candidate, b.candidate);
      if (sort === "availability") result = availability(b.candidate) - availability(a.candidate) || compareMatch(a.candidate, b.candidate);
      if (sort === "duration") {
        const left = a.candidate.durationSeconds, right = b.candidate.durationSeconds;
        if (typeof left !== "number" || !Number.isFinite(left) || left < 0) result = typeof right !== "number" || !Number.isFinite(right) || right < 0 ? 0 : 1;
        else if (typeof right !== "number" || !Number.isFinite(right) || right < 0) result = -1;
        else result = (left - right) * (direction === "desc" ? -1 : 1);
      }
      return result || a.index - b.index;
    });
    return { provider, candidates: ordered.map(({ candidate }) => candidate) };
  });
}

export function candidateFormatsLabel(candidate: Candidate) {
  return formats(candidate).map((format) => format.codec || format.extension).filter(Boolean).map((value) => value.toUpperCase()).filter((value, index, all) => all.indexOf(value) === index).join(" · ");
}

export function formatSourceSize(bytes?: number | null) {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes <= 0) return "Size unknown";
  if (bytes >= 1_073_741_824) return `${(bytes / 1_073_741_824).toFixed(1)} GB`;
  if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(1)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}
