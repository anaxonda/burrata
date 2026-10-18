export interface AvailabilityPage<T> {
  sortPending?: boolean;
  sortAvailability?: T | null;
  sortAvailabilityPending?: boolean;
}

export function sortAvailabilityIsPending(page: { sortAvailabilityPending?: boolean; sortAvailability?: unknown }): boolean {
  return page.sortAvailabilityPending === true;
}

/** Coalesces readiness events and ignores replies after the active query changes. */
export class PendingSortAvailabilityRefresh<T> {
  private eventRevision = 0;
  private running = new Set<string>();

  get revision(): number { return this.eventRevision; }

  noteUpdated(): void { this.eventRevision++; }

  async refresh(
    key: string,
    isPending: () => boolean,
    isCurrent: () => boolean,
    fetch: () => Promise<AvailabilityPage<T>>,
    apply: (page: AvailabilityPage<T>) => void,
  ): Promise<void> {
    if (!isPending() || this.running.has(key)) return;
    this.running.add(key);
    try {
      while (isPending()) {
        const revisionAtRequest = this.eventRevision;
        const page = await fetch();
        if (!isCurrent()) return;
        apply(page);
        if (!isPending() || this.eventRevision === revisionAtRequest) return;
      }
    } finally {
      this.running.delete(key);
    }
  }
}
