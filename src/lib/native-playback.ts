import type { NativePlaybackSnapshot, Track, Candidate } from "./types";
import { classifyPlaybackFailure, type PlaybackFailure } from "./playback-failure";

import type { TrackSortField, SortDirection } from "./track-sort";

/** Typed native details win; older backends retain the string fallback. */
export function nativeSnapshotFailure(snapshot: Pick<NativePlaybackSnapshot, "status" | "playerMessage" | "failure"> | null | undefined): PlaybackFailure | null {
  if (!snapshot || snapshot.status !== "failed") return null;
  return snapshot.failure ?? classifyPlaybackFailure(snapshot.playerMessage);
}

export interface PlaybackOperationTrace { operationId: string; clientStartedAtMs: number }
export function playbackOperationTrace(): PlaybackOperationTrace {
  return {operationId:crypto.randomUUID(),clientStartedAtMs:performance.timeOrigin+performance.now()};
}
export type NativeCommand = NativeCommandPayload & Partial<PlaybackOperationTrace>;
type NativeCommandPayload =
  | { action: "play"; tracks: Track[]; index: number; candidate?: Candidate; position?: number }
  | { action: "queue"; tracks: Track[]; mode: "append" | "replace" | "next"; preserveCurrent?: boolean }
  | { action: "next" | "previous" }
  | { action: "select" | "remove"; index: number }
  | { action: "sort_upcoming"; field: TrackSortField; direction: SortDirection }
  | { action: "reorder"; from: number; to: number; expectedQueueRevision?: number }
  | { action: "pause" | "resume" | "stop" }
  | { action: "seek"; position: number }
  | { action: "volume"; volume: number }
  | { action: "shuffle"; enabled: boolean }
  | { action: "repeat"; mode: "off" | "all" | "one" }
  | { action: "restore"; tracks: Track[]; index: number; position: number; volume: number; repeat: string; shuffle: boolean };

export interface NativePositionEvent {
  generation: number;
  revision?: number;
  position: number;
  duration?: number;
  buffering?: boolean;
  chapter?: NativePlaybackSnapshot["chapter"];
  quality?: NativePlaybackSnapshot["quality"];
}
export type NativePlaybackEvent = (snapshot: NativePlaybackSnapshot) => void;
export type NativeStateEvent = Omit<NativePlaybackSnapshot, "queue"> & { queue?: Track[] };

/** Keep the native bridge small and injectable so its event ordering is testable. */
export class NativePlaybackBridge {
  snapshot: NativePlaybackSnapshot | null = null;
  private stopState?: () => void;
  private stopPosition?: () => void;
  private pendingState: NativeStateEvent | null = null;
  private lifecycle = 0;
  private refreshingQueue = false;

  constructor(
    private invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>,
    private listen: <T>(event: string, handler: (event: { payload: T }) => void) => Promise<() => void>,
  ) {}

  async initialize(onState: NativePlaybackEvent, onPosition: (event: NativePositionEvent) => void): Promise<NativePlaybackSnapshot> {
    this.dispose();
    this.snapshot = null;
    this.pendingState = null;
    const lifecycle = this.lifecycle;
    let receivedStateEvent = false;
    try {
      this.stopState = await this.listen<NativeStateEvent>("native-playback-state", ({ payload }) => {
        if (this.acceptSnapshot(payload)) { receivedStateEvent = true; onState(this.snapshot!); }
        else if (this.snapshot && this.pendingState && !this.refreshingQueue) {
          this.refreshingQueue = true;
          void this.invoke<NativePlaybackSnapshot>("get_playback_snapshot").then(full => {
            if (lifecycle === this.lifecycle && this.acceptSnapshot(full)) onState(this.snapshot!);
          }).catch(() => { /* A later full state event can still restore the queue. */ }).finally(() => {
            if (lifecycle === this.lifecycle) this.refreshingQueue = false;
          });
        }
      });
      this.stopPosition = await this.listen<NativePositionEvent>("native-playback-position", ({ payload }) => {
        if (!this.snapshot || payload.generation !== this.snapshot.generation ||
            payload.revision !== undefined && payload.revision < this.snapshot.revision) return;
        this.snapshot = { ...this.snapshot,
          ...(payload.revision !== undefined ? {revision:payload.revision} : {}),
          position:payload.position,
          ...(payload.duration !== undefined ? {duration:payload.duration} : {}),
          ...(payload.chapter !== undefined ? {chapter:payload.chapter} : {}),
          ...(payload.quality !== undefined ? {quality:payload.quality} : {}),
        };
        onPosition(payload);
      });
      let initial = await this.invoke<NativePlaybackSnapshot>("get_playback_snapshot");
      if (!initial || typeof initial !== "object") throw new Error("Native playback snapshot is unavailable");
      const pendingQueue = this.pendingState as NativeStateEvent | null;
      if (pendingQueue?.queueRevision !== undefined && initial.queueRevision !== pendingQueue.queueRevision) {
        initial = await this.invoke<NativePlaybackSnapshot>("get_playback_snapshot");
      }
      const latest = this.snapshot as NativePlaybackSnapshot | null;
      if (!latest || initial.revision > latest.revision || !receivedStateEvent && initial.revision >= latest.revision) {
        this.acceptSnapshot(initial);
        onState(this.snapshot!);
      }
      if (!this.snapshot) throw new Error("Native playback queue is unavailable");
      return this.snapshot;
    } catch (error) {
      this.dispose();
      this.snapshot = null;
      throw error;
    }
  }

  acceptSnapshot(snapshot: NativeStateEvent): boolean {
    snapshot = {...snapshot, failure:nativeSnapshotFailure(snapshot)};
    if (this.snapshot && snapshot.revision < this.snapshot.revision) return false;
    if (!Array.isArray(snapshot.queue)) {
      if (!this.snapshot || snapshot.queueRevision !== undefined && snapshot.queueRevision !== this.snapshot.queueRevision) {
        if (!this.pendingState || snapshot.revision >= this.pendingState.revision) this.pendingState = snapshot;
        return false;
      }
      this.snapshot = {...snapshot,queue:this.snapshot.queue};
    } else {
      this.snapshot = snapshot as NativePlaybackSnapshot;
      const pending = this.pendingState;
      this.pendingState = null;
      if (pending && pending.revision > snapshot.revision) this.acceptSnapshot(pending);
    }
    return true;
  }

  command(command: NativeCommand): Promise<NativePlaybackSnapshot | void> {
    return this.invoke<NativePlaybackSnapshot | void>("playback_command", { command });
  }

  dispose(): void {
    const stops = [this.stopState, this.stopPosition];
    this.stopState = undefined; this.stopPosition = undefined;
    this.pendingState = null;
    this.refreshingQueue = false;
    this.lifecycle++;
    for (const stop of stops) { try { stop?.(); } catch { /* Continue releasing the remaining listener. */ } }
  }
}
