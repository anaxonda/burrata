import { describe, expect, it, vi } from "vitest";
import { verifiedBatchSource } from "./batch";
import type { Candidate } from "./types";
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
describe("selected batch acquisition", () => {
  it("does not trust a search recommendation that inspection downgrades", async () => {
    expect(
      await verifiedBatchSource(
        [candidate("radio-edit")],
        async (c) => ({ ...c, confidence: "review" }),
        () => false,
      ),
    ).toBeUndefined();
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
