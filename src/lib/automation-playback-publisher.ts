export interface AutomationQueueItem { id: string; artist: string; title: string }

/** Stable compact queue fingerprint, evaluated only when the Svelte derived projection changes. */
export function queueProjectionRevision(rows: AutomationQueueItem[], entryIds: readonly string[] = []): string {
  let a=0x811c9dc5,b=0x9e3779b9,c=0x85ebca6b,d=0xc2b2ae35;
  const feed=(value:string)=>{const framed=`${value.length}:`;for(const text of [framed,value,";"]){for(let i=0;i<text.length;i++){const x=text.charCodeAt(i);a=Math.imul(a^x,0x01000193);b=Math.imul(b^x,0x85ebca6b);c=Math.imul(c^x,0xc2b2ae35);d=Math.imul(d^x,0x27d4eb2f);}}};
  for(let index=0;index<rows.length;index++){const row=rows[index]!;feed(row.id);feed(row.artist);feed(row.title);feed(entryIds[index] ?? "");}
  return `${rows.length}:${a>>>0}:${b>>>0}:${c>>>0}:${d>>>0}`;
}

export interface PlaybackStateAck { protocolVersion?: number; accepted?: boolean; needFull?: boolean; failureHistoryRevision?: string }
export interface PlaybackFailureHistoryProjection { revision: string; failures: readonly unknown[] }
export interface PlaybackEnvelope<TState, TQueue> {
  stateSequence: number;
  queueRevision: string;
  kind: "full" | "unchanged";
  failureHistoryRevision?: string;
  failureHistoryKind?: "full" | "unchanged";
  playback: TState & Record<string, unknown>;
}

/** Tracks only publisher ordering; queue projection/revision stay cached in Svelte derived state. */
export function createAutomationPlaybackPublisher(clientId: string) {
  let sequence = 0;
  let latestAckSequence = 0;
  let acknowledgedRevision: string | null = null;
  let acknowledgedFailureHistory: {queueRevision: string; revision: string} | null = null;
  return {
    next<TState extends Record<string, unknown>, TQueue>(state: TState, revision: string, queue: TQueue[], forceFull = false, queueEntryIds?: readonly string[], history?: PlaybackFailureHistoryProjection): PlaybackEnvelope<TState, TQueue> {
      const stateSequence = ++sequence;
      const kind = forceFull || acknowledgedRevision !== revision ? "full" : "unchanged";
      const failureHistoryKind = history
        ? forceFull || kind === "full" || acknowledgedFailureHistory?.queueRevision !== revision || acknowledgedFailureHistory.revision !== history.revision ? "full" : "unchanged"
        : undefined;
      // The immutable history projection is never traversed or hashed on a pulse.
      const historyFields: Record<string, unknown> = {};
      if (history) {
        const diagnostics = {...(state.diagnostics as Record<string, unknown> | undefined)};
        delete diagnostics.recentTrackFailures;
        if (failureHistoryKind === "full") diagnostics.recentTrackFailures = history.failures;
        historyFields.diagnostics = diagnostics;
        historyFields.failureHistoryRevision = history.revision;
        historyFields.failureHistoryUpdate = failureHistoryKind === "full" ? {kind: "full"} : {kind: "unchanged", failureHistoryRevision: history.revision};
      }
      const playback = {
        ...state,
        ...historyFields,
        protocolVersion: 1,
        clientId,
        stateSequence,
        queueRevision: revision,
        queueUpdate: kind === "full" ? { kind } : { kind, queueRevision: revision },
        ...(kind === "full" ? { queue, ...(queueEntryIds ? { queueEntryIds: [...queueEntryIds] } : {}) } : {}),
      } as TState & Record<string, unknown>;
      return { stateSequence, queueRevision: revision, kind, failureHistoryRevision: history?.revision, failureHistoryKind, playback };
    },
    acknowledge(envelope: PlaybackEnvelope<unknown, unknown>, ack: PlaybackStateAck | undefined): "accepted" | "retry_full" | "rejected" | "legacy" {
      if (ack?.protocolVersion !== 1) return "legacy";
      if (ack.accepted && envelope.stateSequence >= latestAckSequence) {
        latestAckSequence = envelope.stateSequence;
        if (envelope.kind === "full") acknowledgedRevision = envelope.queueRevision;
        if (envelope.failureHistoryKind === "full" && envelope.failureHistoryRevision !== undefined && ack.failureHistoryRevision === envelope.failureHistoryRevision) {
          acknowledgedFailureHistory = {queueRevision: envelope.queueRevision, revision: envelope.failureHistoryRevision};
        }
        return "accepted";
      }
      if (ack.needFull && envelope.stateSequence >= latestAckSequence) {
        latestAckSequence = envelope.stateSequence;
        acknowledgedRevision = null;
        acknowledgedFailureHistory = null;
        return "retry_full";
      }
      return "rejected";
    },
  };
}
