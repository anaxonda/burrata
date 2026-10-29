const DAY_MS = 24 * 60 * 60 * 1000;

export type HistoryOccurrenceRange = { from: string; to: string };

/** Build plain UTC calendar-date bounds that include the occurrence's full UTC day. */
export function historyOccurrenceDateRange(playedAt: string): HistoryOccurrenceRange | null {
  const instant = Date.parse(playedAt);
  if (!Number.isFinite(instant)) return null;

  try {
    const utcDay = new Date(instant).toISOString().slice(0, 10);
    const dayStart = Date.parse(`${utcDay}T00:00:00.000Z`);
    const from = new Date(dayStart - DAY_MS).toISOString().slice(0, 10);
    const to = new Date(dayStart + DAY_MS).toISOString().slice(0, 10);
    if (![utcDay, from, to].every(value => /^\d{4}-\d{2}-\d{2}$/.test(value))) return null;
    return { from, to };
  } catch {
    return null;
  }
}
