import { describe, expect, it } from "vitest";
import {
  emptyRecurringStallRecoveryState,
  recordHealthyPlaybackProgress,
  recordPlaybackStall,
} from "./recurring-stall-recovery";

describe("recurring stall recovery", () => {
  it("requests candidate replacement on the second same-candidate stall within 90 seconds", () => {
    const first = recordPlaybackStall(emptyRecurringStallRecoveryState(), "youtube:one", 1_000);
    expect(first.replaceCandidate).toBe(false);
    expect(recordPlaybackStall(first.state, "youtube:one", 91_000)).toMatchObject({ replaceCandidate: true });
  });

  it("does not count a stall outside the 90-second window or for another candidate", () => {
    const first = recordPlaybackStall(emptyRecurringStallRecoveryState(), "youtube:one", 1_000);
    const tooLate = recordPlaybackStall(first.state, "youtube:one", 91_001);
    expect(tooLate.replaceCandidate).toBe(false);
    expect(tooLate.state.stallTimesMs).toEqual([91_001]);
    const switched = recordPlaybackStall(first.state, "youtube:two", 2_000);
    expect(switched.replaceCandidate).toBe(false);
    expect(switched.state.stallTimesMs).toEqual([2_000]);
  });

  it("consumes a triggered pair so duplicate notifications do not repeatedly request replacement", () => {
    const first = recordPlaybackStall(emptyRecurringStallRecoveryState(), "youtube:one", 1_000);
    const second = recordPlaybackStall(first.state, "youtube:one", 2_000);
    expect(second.replaceCandidate).toBe(true);
    const duplicate = recordPlaybackStall(second.state, "youtube:one", 2_001);
    expect(duplicate.replaceCandidate).toBe(false);
  });

  it("clears failure history only after 60 seconds of observed healthy progress", () => {
    const stalled = recordPlaybackStall(emptyRecurringStallRecoveryState(), "youtube:one", 1_000).state;
    const healthy = recordHealthyPlaybackProgress(stalled, "youtube:one", 10_000);
    expect(healthy.stallTimesMs).toEqual([1_000]);
    const beforeReset = recordHealthyPlaybackProgress(healthy, "youtube:one", 69_999);
    expect(beforeReset.stallTimesMs).toEqual([1_000]);
    const reset = recordHealthyPlaybackProgress(beforeReset, "youtube:one", 70_000);
    expect(reset.stallTimesMs).toEqual([]);
  });

  it("starts a new healthy interval after a stall or candidate change", () => {
    const first = recordHealthyPlaybackProgress(emptyRecurringStallRecoveryState(), "youtube:one", 1_000);
    const stalled = recordPlaybackStall(first, "youtube:one", 30_000).state;
    expect(stalled.healthySinceMs).toBeNull();
    const resumed = recordHealthyPlaybackProgress(stalled, "youtube:one", 31_000);
    expect(resumed.healthySinceMs).toBe(31_000);
    const switched = recordHealthyPlaybackProgress(resumed, "youtube:two", 32_000);
    expect(switched).toEqual({ candidateKey: "youtube:two", stallTimesMs: [], healthySinceMs: 32_000 });
  });
});
