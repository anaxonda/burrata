export type WebviewMediaEventType =
  | "play" | "playing" | "waiting" | "stalled" | "canplay" | "canplaythrough"
  | "seeking" | "seeked" | "ended" | "error";
export type WebviewMediaEventOwner = {
  sessionId: string | null;
  sourceGeneration: number | null;
  entryId: string | null;
  seekSerial: number | null;
};
type BufferedRange = { start: number; end: number };
type WebviewMediaEvent = {
  atMs: number | null;
  type: WebviewMediaEventType;
  readyState: number | null;
  networkState: number | null;
  paused: boolean | null;
  mediaErrorCode?: number;
  currentTime: number | null;
  durationSeconds: number | null;
  webviewGeneration: number | null;
  buffered: BufferedRange[];
};
export type WebviewDiagnosticHistoryEvent = WebviewMediaEvent & { attempt: number };
type MediaSample = Pick<HTMLMediaElement, "readyState" | "networkState" | "paused" | "currentTime" | "duration"> & {
  error?: Pick<MediaError, "code"> | null;
  buffered?: Pick<TimeRanges, "length" | "start" | "end"> | null;
};
const MEDIA_EVENTS = new Set<WebviewMediaEventType>([
  'play', 'playing', 'waiting', 'stalled', 'canplay', 'canplaythrough',
  'seeking', 'seeked', 'ended', 'error',
]);
const MAX_EVENTS = 64;
const MAX_RANGES = 8;
const MAX_ATTEMPT = 1_000_000;

function finiteNonnegative(value: number) {
  return Number.isFinite(value) && value >= 0 ? value : null;
}

function boundedDurationSeconds(value: number) {
  return Number.isFinite(value) && value >= 0 && value <= 86_400 ? value : null;
}

function ownerKey(owner: WebviewMediaEventOwner) {
  return JSON.stringify([
    owner?.sessionId ?? null,
    owner?.sourceGeneration ?? null,
    owner?.entryId ?? null,
    owner?.seekSerial ?? null,
  ]);
}

export function createWebviewMediaEventRecorder({
  enabled = false,
  now = () => performance.now(),
  capacity = MAX_EVENTS,
}: { enabled?: boolean; now?: () => number; capacity?: number } = {}) {
  const limit = Number.isInteger(capacity) ? Math.max(1, Math.min(MAX_EVENTS, capacity)) : MAX_EVENTS;
  let active = Boolean(enabled);
  let currentKey: string | null = null;
  let currentOwner: WebviewMediaEventOwner | null = null;
  let events: WebviewMediaEvent[] = [];
  let attempt = 0;
  let history: WebviewDiagnosticHistoryEvent[] = [];

  return {
    get enabled() { return active; },
    setEnabled(value: boolean) { active = Boolean(value); },
    beginAttempt() {
      attempt = Math.min(MAX_ATTEMPT, attempt + 1);
      currentKey = null; currentOwner = null; events = [];
      return attempt;
    },
    record(owner: WebviewMediaEventOwner, type: WebviewMediaEventType | string, media: MediaSample) {
      if (!active || !owner || !MEDIA_EVENTS.has(type as WebviewMediaEventType) || !media) return;
      const eventType = type as WebviewMediaEventType;
      const key = ownerKey(owner);
      if (key !== currentKey) {
        if (attempt === 0) attempt = 1;
        currentKey = key;
        currentOwner = {
          sessionId: typeof owner.sessionId === 'string' ? owner.sessionId.slice(0, 128) : null,
          sourceGeneration: typeof owner.sourceGeneration === "number" && Number.isSafeInteger(owner.sourceGeneration) && owner.sourceGeneration >= 0 ? owner.sourceGeneration : null,
          entryId: typeof owner.entryId === 'string' ? owner.entryId.slice(0, 128) : null,
          seekSerial: typeof owner.seekSerial === "number" && Number.isSafeInteger(owner.seekSerial) && owner.seekSerial >= 0 ? owner.seekSerial : null,
        };
        events = [];
      }
      const buffered: BufferedRange[] = [];
      try {
        const ranges = media.buffered;
        const count = Math.min(MAX_RANGES, Number(ranges?.length) || 0);
        for (let index = 0; ranges && index < count; index += 1) {
          const start = finiteNonnegative(ranges.start(index));
          const end = finiteNonnegative(ranges.end(index));
          if (start !== null && end !== null && end >= start) buffered.push({ start, end });
        }
      } catch { /* A detached media element can invalidate TimeRanges during sampling. */ }
      const timestamp = now();
      const event: WebviewMediaEvent = {
        atMs: Number.isFinite(timestamp) && timestamp >= 0 ? timestamp : null,
        type: eventType,
        readyState: Number.isInteger(media.readyState) && media.readyState >= 0 && media.readyState <= 4 ? media.readyState : null,
        networkState: Number.isInteger(media.networkState) && media.networkState >= 0 && media.networkState <= 3 ? media.networkState : null,
        paused: typeof media.paused === 'boolean' ? media.paused : null,
        currentTime: finiteNonnegative(media.currentTime),
        durationSeconds: boundedDurationSeconds(media.duration),
        webviewGeneration: typeof owner.sourceGeneration === "number" && Number.isSafeInteger(owner.sourceGeneration) && owner.sourceGeneration >= 0
          ? owner.sourceGeneration : null,
        buffered,
      };
      if (Number.isInteger(media.error?.code) && media.error!.code >= 1 && media.error!.code <= 4) event.mediaErrorCode = media.error!.code;
      events.push(event);
      history.push({...event, attempt});
      if (events.length > limit) events.splice(0, events.length - limit);
      if (history.length > MAX_EVENTS) history.splice(0, history.length - MAX_EVENTS);
    },
    snapshot(owner: WebviewMediaEventOwner) {
      if (!active || !owner) return null;
      if (ownerKey(owner) !== currentKey) {
        currentKey = null;
        currentOwner = null;
        events = [];
        return null;
      }
      return { owner: currentOwner!, events: events.map(event => ({ ...event, buffered: event.buffered.map(range => ({ ...range })) })) };
    },
    snapshotLatest() {
      if (!currentKey) return null;
      return events.map(event => ({ ...event, buffered: event.buffered.map(range => ({ ...range })) }));
    },
    snapshotHistory() {
      return history.map(event => ({...event, buffered:event.buffered.map(range=>({...range}))}));
    },
    reset() {
      currentKey = null;
      currentOwner = null;
      events = [];
      attempt = 0;
      history = [];
    },
  };
}
