/** Resolves only the newest request; obsolete failures must not replace current UI state. */
export class LatestRequest {
  // Keep generations increasing across a WebView reload so the backend can
  // reject an older IPC that arrives after a newer preview request.
  private generation = Date.now() * 1000;
  next(): number {
    return ++this.generation;
  }
  isCurrent(token: number): boolean {
    return token === this.generation;
  }
  async resolve<T>(
    token: number,
    request: Promise<T>,
    onStale?: (value: T) => void,
  ): Promise<T | undefined> {
    try {
      const value = await request;
      if (this.isCurrent(token)) return value;
      onStale?.(value);
      return undefined;
    } catch (error) {
      if (this.isCurrent(token)) throw error;
      return undefined;
    }
  }
}
