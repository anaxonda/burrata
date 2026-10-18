export type ProviderReadCall = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

function requestUuid(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = [...bytes].map(value => value.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** Owns cancellation only for reads issued by this UI instance. */
export class ProviderReadCancellation {
  private readonly active = new Set<string>();
  constructor(private readonly call: ProviderReadCall) {}

  async read<T>(command: string, args: Record<string, unknown>): Promise<T> {
    const requestId = requestUuid();
    this.active.add(requestId);
    try {
      return await this.call(command, { ...args, requestId }) as T;
    } finally {
      this.active.delete(requestId);
    }
  }

  cancelAll(): void {
    const requestIds = [...this.active];
    this.active.clear();
    for (const requestId of requestIds) {
      void this.call("cancel_remote_read", { requestId }).catch(() => {});
    }
  }
}
