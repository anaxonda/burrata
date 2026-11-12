export interface RadioQueueOccurrence<T> {
  id: string;
  occurrenceId?: string;
  sessionId: string;
  stationId: string;
  track: T;
  playedAt: string;
  stationName: string;
}

/** Session detections in playback order, excluding only the current occurrence. */
export function previousRadioDetections<T>(
  rows: RadioQueueOccurrence<T>[],
  sessionId: string | null | undefined,
  stationId: string | null | undefined,
  currentOccurrenceId: string | null | undefined,
): RadioQueueOccurrence<T>[] {
  if (!sessionId || !stationId) return [];
  return rows
    .map((row, index) => ({ row, index }))
    .filter(({ row }) =>
      row.sessionId === sessionId &&
      row.stationId === stationId &&
      (row.occurrenceId ?? row.id) !== currentOccurrenceId
    )
    .sort((a, b) => {
      const time = Date.parse(a.row.playedAt) - Date.parse(b.row.playedAt);
      return (Number.isFinite(time) ? time : 0) || a.index - b.index;
    })
    .map(({ row }) => row);
}
