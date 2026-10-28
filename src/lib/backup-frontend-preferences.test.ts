import { describe, expect, it, vi } from "vitest";
import {
  applyBackupFrontendPreferences,
  BACKUP_FRONTEND_PREFERENCE_KEYS,
  BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY,
  type PendingBackupFrontendPreferences,
} from "./backup-frontend-preferences";

const waveformKey = "burrata.waveform-enabled.v1";
const testEnvelope = (nonce = "10000000-0000-4000-8000-000000000001", currentSessionId = "20000000-0000-4000-8000-000000000001", preferences: Record<string, unknown> = { [waveformKey]: true }): PendingBackupFrontendPreferences => ({
  nonce,
  currentSessionId,
  preferences,
});

function storageFrom(durable = new Map<string, string>(), persist = true) {
  const memory = new Map(durable);
  return {
    memory,
    getItem: (key: string) => memory.get(key) ?? null,
    setItem: (key: string, value: string) => {
      memory.set(key, value);
      if (persist) durable.set(key, value);
    },
  };
}

describe("backup frontend preference hydration", () => {
  it("allowlists preferences, writes a receipt last, and defers same-session acknowledgement", async () => {
    expect(BACKUP_FRONTEND_PREFERENCE_KEYS.has("burrata.stream-media-analysis-enabled.v1")).toBe(true);
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    };
    const calls: string[] = [];
    const ack = vi.fn(async (nonce: string, sessionId: string) => {
      calls.push(`${nonce}:${sessionId}`);
      return "deferred" as const;
    });

    const result = await applyBackupFrontendPreferences(
      async () => testEnvelope("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000001", { [waveformKey]: true, "burrata.unknown.v1": true }),
      storage,
      ack,
    );

    expect(result).toBe("deferred");
    expect(values.get(waveformKey)).toBe("true");
    expect(values.has("burrata.unknown.v1")).toBe(false);
    expect(JSON.parse(values.get(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY) ?? "{}")).toEqual({ nonce: "10000000-0000-4000-8000-000000000001", sessionId: "20000000-0000-4000-8000-000000000001" });
    expect(calls).toEqual(["10000000-0000-4000-8000-000000000001:20000000-0000-4000-8000-000000000001"]);
  });

  it("publishes validated startup preferences after ACK if WebKit later rolls records back", async () => {
    const durable = new Map([[waveformKey, "false"]]);
    const storage = storageFrom(durable, true);
    const previousValue = durable.get(waveformKey)!;
    const applied = vi.fn();
    const ack = vi.fn(async () => {
      storage.memory.set(waveformKey, previousValue);
      return "deferred" as const;
    });

    expect(await applyBackupFrontendPreferences(async () => testEnvelope(), storage, ack, applied)).toBe("deferred");
    expect(storage.memory.get(waveformKey)).toBe("false");
    expect(applied).toHaveBeenCalledWith(new Map([[waveformKey, "true"]]));
    expect(applied.mock.calls[0]![0].has(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY)).toBe(false);
  });

  it("retries after a silent memory-only write and acknowledges only after a later session reads the durable receipt", async () => {
    const durable = new Map([[waveformKey, "false"]]);
    const readonlySession = storageFrom(durable, false);
    const ackSession1 = vi.fn(async () => "deferred" as const);
    expect(await applyBackupFrontendPreferences(async () => testEnvelope(), readonlySession, ackSession1)).toBe("deferred");
    expect(readonlySession.memory.get(waveformKey)).toBe("true");
    expect(durable.get(waveformKey)).toBe("false");
    expect(durable.has(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY)).toBe(false);

    const writableSession = storageFrom(durable, true);
    const ackSession2 = vi.fn(async () => "deferred" as const);
    expect(await applyBackupFrontendPreferences(async () => testEnvelope("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000002"), writableSession, ackSession2)).toBe("deferred");
    expect(durable.get(waveformKey)).toBe("true");
    expect(JSON.parse(durable.get(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY) ?? "{}")).toEqual({ nonce: "10000000-0000-4000-8000-000000000001", sessionId: "20000000-0000-4000-8000-000000000002" });

    const laterSession = storageFrom(durable, true);
    const ackSession3 = vi.fn(async (nonce: string, receiptSession: string) => receiptSession === "20000000-0000-4000-8000-000000000002" && nonce === "10000000-0000-4000-8000-000000000001" ? "acknowledged" as const : "stale" as const);
    expect(await applyBackupFrontendPreferences(async () => testEnvelope("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000003"), laterSession, ackSession3)).toBe("acknowledged");
    expect(ackSession3).toHaveBeenCalledWith("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000002");
    expect(laterSession.memory.get(waveformKey)).toBe("true");
  });

  it("keeps startup pending when storage silently ignores a preference write", async () => {
    const durable = new Map([[waveformKey, "false"]]);
    const setItem = vi.fn();
    const ack = vi.fn(async () => "deferred" as const);
    await expect(applyBackupFrontendPreferences(
      async () => testEnvelope(),
      { getItem: key => durable.get(key) ?? null, setItem },
      ack,
    )).rejects.toThrow("could not be saved");
    expect(durable.get(waveformKey)).toBe("false");
    expect(durable.has(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY)).toBe(false);
    expect(ack).not.toHaveBeenCalled();
  });

  it("acknowledges a durable receipt without overwriting a later intentional preference change", async () => {
    const durable = new Map<string, string>();
    const firstSession = storageFrom(durable, true);
    await applyBackupFrontendPreferences(async () => testEnvelope("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000001"), firstSession, async () => "deferred");
    durable.set(waveformKey, "false"); // The user changed this preference after the import.

    const nextSession = storageFrom(durable, true);
    const setItem = vi.spyOn(nextSession, "setItem");
    const ack = vi.fn(async () => "acknowledged" as const);
    const applied = vi.fn();
    expect(await applyBackupFrontendPreferences(async () => testEnvelope("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000002"), nextSession, ack, applied)).toBe("acknowledged");
    expect(nextSession.memory.get(waveformKey)).toBe("false");
    expect(setItem).not.toHaveBeenCalled();
    expect(applied).not.toHaveBeenCalled();
    expect(ack).toHaveBeenCalledWith("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000001");
  });

  it("replays instead of getting stuck when a matching receipt has a malformed session id", async () => {
    const durable = new Map([
      [waveformKey, "false"],
      [BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY, JSON.stringify({ nonce: "10000000-0000-4000-8000-000000000001", sessionId: "not-a-session" })],
    ]);
    const storage = storageFrom(durable, true);
    const ack = vi.fn(async () => "deferred" as const);

    expect(await applyBackupFrontendPreferences(async () => testEnvelope(), storage, ack)).toBe("deferred");
    expect(storage.memory.get(waveformKey)).toBe("true");
    expect(JSON.parse(storage.memory.get(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY) ?? "{}")).toEqual({
      nonce: "10000000-0000-4000-8000-000000000001",
      sessionId: "20000000-0000-4000-8000-000000000001",
    });
    expect(ack).toHaveBeenCalledTimes(1);
  });

  it("rejects array-shaped pending preferences", async () => {
    const storage = storageFrom();
    const ack = vi.fn(async () => "acknowledged" as const);
    await expect(applyBackupFrontendPreferences(
      async () => ({ ...testEnvelope(), preferences: [] as unknown as Record<string, unknown> }),
      storage,
      ack,
    )).rejects.toThrow("Pending frontend preferences are invalid");
    expect(ack).not.toHaveBeenCalled();
  });

  it("refetches and applies a newer envelope when an older receipt acknowledgement is stale", async () => {
    const storage = storageFrom(new Map([
      [waveformKey, "false"],
      [BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY, JSON.stringify({ nonce: "10000000-0000-4000-8000-000000000003", sessionId: "20000000-0000-4000-8000-000000000001" })],
    ]), true);
    const getPending = vi.fn()
      .mockResolvedValueOnce(testEnvelope("10000000-0000-4000-8000-000000000003", "20000000-0000-4000-8000-000000000002", { [waveformKey]: true }))
      .mockResolvedValueOnce(testEnvelope("10000000-0000-4000-8000-000000000004", "20000000-0000-4000-8000-000000000002", { [waveformKey]: false }));
    const ack = vi.fn()
      .mockResolvedValueOnce("stale" as const)
      .mockResolvedValueOnce("deferred" as const);

    expect(await applyBackupFrontendPreferences(getPending, storage, ack)).toBe("deferred");
    expect(getPending).toHaveBeenCalledTimes(2);
    expect(ack).toHaveBeenNthCalledWith(1, "10000000-0000-4000-8000-000000000003", "20000000-0000-4000-8000-000000000001");
    expect(ack).toHaveBeenNthCalledWith(2, "10000000-0000-4000-8000-000000000004", "20000000-0000-4000-8000-000000000002");
    expect(storage.memory.get(waveformKey)).toBe("false");
    expect(JSON.parse(storage.memory.get(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY) ?? "{}")).toEqual({ nonce: "10000000-0000-4000-8000-000000000004", sessionId: "20000000-0000-4000-8000-000000000002" });
  });

  it("keeps the native envelope pending when storage throws", async () => {
    const setItem = vi.fn(() => { throw new Error("storage write failed"); });
    const ack = vi.fn(async () => "acknowledged" as const);
    await expect(applyBackupFrontendPreferences(async () => testEnvelope(), { getItem: () => null, setItem }, ack)).rejects.toThrow("storage write failed");
    expect(ack).not.toHaveBeenCalled();
  });

  it("rejects malformed restored values without acknowledging the envelope", async () => {
    const storage = storageFrom();
    const ack = vi.fn(async () => "acknowledged" as const);
    await expect(applyBackupFrontendPreferences(
      async () => testEnvelope("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000001", { [waveformKey]: "true" }),
      storage,
      ack,
    )).rejects.toThrow("Invalid restored value");
    expect(storage.memory.has(waveformKey)).toBe(false);
    expect(ack).not.toHaveBeenCalled();
  });

  it("does nothing when there is no pending envelope", async () => {
    const storage = { getItem: () => null, setItem: vi.fn() };
    const ack = vi.fn();
    expect(await applyBackupFrontendPreferences(async () => null, storage, ack)).toBe("none");
    expect(storage.setItem).not.toHaveBeenCalled();
    expect(ack).not.toHaveBeenCalled();
  });
});
