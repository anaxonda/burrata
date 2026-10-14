import type { Candidate } from "./types";
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
