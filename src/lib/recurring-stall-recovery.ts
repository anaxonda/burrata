/** Per-candidate transport history used to avoid retrying a repeatedly failing source forever. */
export type RecurringStallRecoveryState = {
  candidateKey: string | null;
  stallTimesMs: number[];
  healthySinceMs: number | null;
};

export const RECURRING_STALL_WINDOW_MS = 90_000;
export const HEALTHY_PLAYBACK_RESET_MS = 60_000;

export function emptyRecurringStallRecoveryState(): RecurringStallRecoveryState {
  return { candidateKey: null, stallTimesMs: [], healthySinceMs: null };
}

/** Record a stall; a second stall for this candidate within 90 seconds asks the caller to replace it. */
export function recordPlaybackStall(
  previous: RecurringStallRecoveryState,
  candidateKey: string,
  nowMs: number,
): { state: RecurringStallRecoveryState; replaceCandidate: boolean } {
  if (!candidateKey || !Number.isFinite(nowMs) || nowMs < 0) {
    return { state: cloneState(previous), replaceCandidate: false };
  }

  const sameCandidate = previous.candidateKey === candidateKey;
  const priorStalls = sameCandidate ? previous.stallTimesMs : [];
  const inWindow = priorStalls.filter(time => Number.isFinite(time) && time <= nowMs && nowMs - time <= RECURRING_STALL_WINDOW_MS);
  const stalls = [...inWindow, nowMs];
  const replaceCandidate = stalls.length >= 2;
  return {
    // Consume a triggered pair so repeated stall notifications cannot keep
    // requesting replacement without another new failure.
    state: {
      candidateKey,
      stallTimesMs: replaceCandidate ? [] : stalls,
      healthySinceMs: null,
    },
    replaceCandidate,
  };
}

/** Call only after observing real forward media-time progress. */
export function recordHealthyPlaybackProgress(
  previous: RecurringStallRecoveryState,
  candidateKey: string,
  nowMs: number,
): RecurringStallRecoveryState {
  if (!candidateKey || !Number.isFinite(nowMs) || nowMs < 0) return cloneState(previous);
  if (previous.candidateKey !== candidateKey) {
    return { candidateKey, stallTimesMs: [], healthySinceMs: nowMs };
  }

  const healthySinceMs = previous.healthySinceMs;
  if (healthySinceMs === null || !Number.isFinite(healthySinceMs) || nowMs < healthySinceMs) {
    return { ...cloneState(previous), healthySinceMs: nowMs };
  }
  if (nowMs - healthySinceMs >= HEALTHY_PLAYBACK_RESET_MS) {
    return { candidateKey, stallTimesMs: [], healthySinceMs: nowMs };
  }
  return { ...cloneState(previous), healthySinceMs };
}

function cloneState(state: RecurringStallRecoveryState): RecurringStallRecoveryState {
  return {
    candidateKey: state.candidateKey,
    stallTimesMs: [...state.stallTimesMs],
    healthySinceMs: state.healthySinceMs,
  };
}
