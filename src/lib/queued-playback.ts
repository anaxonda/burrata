/** Playback ownership survives RPC completion and receipt delivery failures. */
export interface QueuedPlayRequest {
  operationId: string;
  requestId: string;
  trackIds: string[];
}

export interface QueuedPlayReceipt {
  engine: "webview";
  trackIds: string[];
  queueRevision: string;
  queueEntryIds: string[];
  acceptedGeneration: number;
}

export interface QueuedPlayOutcome {
  status: "playing" | "paused" | "failed" | "exhausted" | "cancelled" | "superseded";
  detail: Record<string, unknown>;
}

export interface QueuedPlayDependencies<T extends { id: string }> {
  load(ids: string[]): Promise<T[]>;
  current(): boolean;
  prepare(tracks: T[]): {
    receipt: QueuedPlayReceipt;
    completion: Promise<QueuedPlayOutcome>;
  };
  accept(receipt: QueuedPlayReceipt): Promise<{ accepted: boolean }>;
  lookup(): Promise<{ status: string; receipt?: unknown }>;
  stopOwned(): void;
  update(outcome: QueuedPlayOutcome): Promise<unknown>;
}

export function exactQueuedTracks<T extends { id: string }>(ids: string[], loaded: T[]): T[] {
  if (!ids.length || ids.some(id => typeof id !== "string" || !id.trim()) ||
      !Array.isArray(loaded) || loaded.length !== ids.length) {
    throw new Error("Unknown or empty track IDs");
  }
  const byId = new Map<string, T[]>();
  for (const track of loaded) {
    if (!track || typeof track.id !== "string") {
      throw new Error("Unknown or empty track IDs");
    }
    const rows = byId.get(track.id) ?? [];
    rows.push(track);
    byId.set(track.id, rows);
  }
  const result = ids.map(id => byId.get(id)?.shift());
  if (result.some(track => !track) || [...byId.values()].some(rows => rows.length)) {
    throw new Error("Unknown or empty track IDs");
  }
  return result as T[];
}

export async function dispatchQueuedPlayback<T extends { id: string }>(
  request: QueuedPlayRequest,
  deps: QueuedPlayDependencies<T>,
): Promise<void> {
  // App's update dependency retains undelivered reports in its bounded outbox.
  const report = (outcome: QueuedPlayOutcome) => deps.update(outcome).catch(() => {});
  try {
    const tracks = exactQueuedTracks(request.trackIds, await deps.load(request.trackIds));
    if (!deps.current()) {
      await report({ status: "superseded", detail: {} });
      return;
    }
    const { receipt, completion } = deps.prepare(tracks);
    // Observe startup rejection before awaiting the independent acceptance IPC.
    const settled = completion.catch(error => ({
      status: "failed" as const,
      detail: { reason: String(error) },
    }));
    let accepted: boolean | undefined;
    try {
      accepted = (await deps.accept(receipt)).accepted;
    } catch {
      try {
        const record = await deps.lookup();
        if (record.receipt) {
          accepted = !["cancelled", "superseded"].includes(record.status);
        } else if (record.status !== "preparing") {
          accepted = false;
        }
      } catch {
        // Receipt delivery remains ambiguous; ownership cannot be revoked here.
      }
    }
    if (accepted === false) {
      if (deps.current()) deps.stopOwned();
      return;
    }
    // A finite backend prepare deadline still cancels a never-accepted operation.
    void settled.then(outcome => report(
      deps.current() ? outcome : { status: "superseded", detail: {} },
    ));
  } catch (error) {
    await report({
      status: deps.current() ? "failed" : "superseded",
      detail: { reason: String(error) },
    });
  }
}

/** Retain failed status deliveries for later automation pulses. Reservations
 * prevent accepting more WebView work than the bounded outbox can retain. */
export function createQueuedPlaybackOutbox(
  send: (request: QueuedPlayRequest, outcome: QueuedPlayOutcome) => Promise<{ accepted: boolean }>,
  capacity = 128,
) {
  type Pending = { request: QueuedPlayRequest; outcome: QueuedPlayOutcome };
  const pending = new Map<string, Pending>();
  const reserved = new Set<string>();
  let flushing: Promise<void> | null = null;
  const key = (request: QueuedPlayRequest) => `${request.operationId}:${request.requestId}`;
  const reserve = (request: QueuedPlayRequest) => {
    const id = key(request);
    if (reserved.has(id)) return true;
    if (reserved.size >= capacity) return false;
    reserved.add(id);
    return true;
  };
  const flush = (): Promise<void> => {
    if (flushing) return flushing;
    flushing = (async () => {
      for (const [id, entry] of pending) {
        for (let attempt = 0; attempt < 3; attempt++) {
          try {
            await send(entry.request, entry.outcome);
            if (pending.get(id) === entry) {
              pending.delete(id);
              reserved.delete(id);
            }
            break;
          } catch {
            // Keep the entry after bounded retries; the one-second pulse retries.
          }
        }
      }
    })().finally(() => { flushing = null; });
    return flushing;
  };
  return {
    reserve,
    flush,
    pendingCount: () => pending.size,
    async publish(request: QueuedPlayRequest, outcome: QueuedPlayOutcome): Promise<void> {
      if (!reserve(request)) throw Error("Playback operation status backlog is full");
      const id = key(request);
      const previous = pending.get(id);
      const active = (status: QueuedPlayOutcome["status"]) => status === "playing" || status === "paused";
      if (previous && !active(previous.outcome.status) && active(outcome.status)) return;
      pending.set(id, { request, outcome });
      await flush();
    },
  };
}
