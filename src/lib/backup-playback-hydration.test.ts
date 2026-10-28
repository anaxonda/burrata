import { describe, expect, it, vi } from "vitest";
import {
  applyPendingBackupPlaybackRestore,
  BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY,
  type PendingBackupPlaybackRestore,
} from "./backup-playback-hydration";
import { consumeStartupPlaybackControls, readPlaybackControls } from "./playback-controls";
import { startAfterBackupHydration } from "./backup-startup-gate";

const nonce = "a1b2c3d4-e5f6-4711-8222-334455667788";
const session1 = "20000000-0000-4000-8000-000000000001";
const session2 = "20000000-0000-4000-8000-000000000002";
const session3 = "20000000-0000-4000-8000-000000000003";

const handoff = (currentSessionId = session1, id = nonce): PendingBackupPlaybackRestore => ({
  version: 1, nonce: id, currentSessionId,
  queue: { tracks: [{ id: "local:track", localPath: "/restored/root/audio.wav" } as never], entryIds: ["occurrence-a"], currentIndex: 0, repeat: "all" },
  position: 12.5, volume: 0, repeat: "all", shuffle: true, paused: true,
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

describe("selected backup playback hydration", () => {
  it("writes the paused queue, position and controls before receipt, then defers same-session ack", async () => {
    const pending = handoff();
    const storage = storageFrom();
    const events: string[] = [];
    const rawSet = storage.setItem;
    const set = vi.fn((key: string, value: string) => { events.push(`set:${key}`); rawSet(key, value); });
    const ack = vi.fn(async (ackNonce: string, receiptSessionId: string) => {
      events.push(`ack:${ackNonce}:${receiptSessionId}`);
      return "deferred" as const;
    });
    const applied = vi.fn();
    expect(await applyPendingBackupPlaybackRestore(async () => pending, { ...storage, setItem: set }, ack, applied)).toBe(true);
    expect(JSON.parse(storage.memory.get("burrata.playback-queue.v1")!).entryIds).toEqual(["occurrence-a"]);
    expect(JSON.parse(storage.memory.get("burrata.playback-position.v1")!)).toMatchObject({ trackId: "local:track", position: 12.5 });
    expect(JSON.parse(storage.memory.get("burrata.playback-controls.v1")!)).toEqual({ volume: 0, shuffle: true });
    expect(JSON.parse(storage.memory.get("burrata.engine-switch-controls.v1")!)).toEqual({ volume: 0, shuffle: true });
    expect(JSON.parse(storage.memory.get(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)!)).toEqual({ nonce, sessionId: session1 });
    expect(readPlaybackControls({ getItem: key => storage.memory.get(key) ?? null } as Storage)).toEqual({ volume: 0, shuffle: true });
    expect(events[events.length - 2]).toBe(`set:${BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY}`);
    expect(events[events.length - 1]).toBe(`ack:${nonce}:${session1}`);
    expect(ack).toHaveBeenCalledWith(nonce, session1);
    expect(applied).toHaveBeenCalledOnce();
    expect(applied.mock.calls[0]![0].has(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)).toBe(false);
  });

  it("publishes startup values after ACK even when existing WebKit records roll back", async () => {
    const durable = new Map([
      ["burrata.playback-queue.v1", "old queue"],
      ["burrata.playback-position.v1", "old position"],
      ["burrata.playback-controls.v1", "old controls"],
      ["burrata.engine-switch-controls.v1", "old engine controls"],
    ]);
    const storage = storageFrom(durable, true);
    const previous = new Map(durable);
    const applied = vi.fn();
    const ack = vi.fn(async () => {
      // Model a later WebKit rollback after immediate getItem verification.
      for (const [key, value] of previous) storage.memory.set(key, value);
      return "deferred" as const;
    });

    expect(await applyPendingBackupPlaybackRestore(async () => handoff(), storage, ack, applied)).toBe(true);
    expect(storage.memory.get("burrata.playback-queue.v1")).toBe("old queue");
    expect(storage.memory.get("burrata.playback-controls.v1")).toBe("old controls");
    const values = applied.mock.calls[0]![0];
    expect(JSON.parse(values.get("burrata.playback-queue.v1")!).entryIds).toEqual(["occurrence-a"]);
    expect(JSON.parse(values.get("burrata.playback-controls.v1")!)).toEqual({ volume: 0, shuffle: true });
    expect(values.has(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)).toBe(false);
  });

  it("retries a silent memory-only write after restart, then preserves later user edits on receipt ack", async () => {
    const durable = new Map<string, string>([
      ["burrata.playback-queue.v1", "old queue"],
      ["burrata.playback-position.v1", "old position"],
      ["burrata.playback-controls.v1", JSON.stringify({ volume: 0.4, shuffle: false })],
    ]);
    const readonlyWebview = storageFrom(durable, false);
    const pending = handoff(session1);
    const sameSessionAck = vi.fn(async () => "deferred" as const);
    expect(await applyPendingBackupPlaybackRestore(async () => pending, readonlyWebview, sameSessionAck)).toBe(true);
    expect(readonlyWebview.memory.get("burrata.playback-queue.v1")).toContain("occurrence-a");
    expect(durable.get("burrata.playback-queue.v1")).toBe("old queue");
    expect(durable.has(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)).toBe(false);
    expect(sameSessionAck).toHaveBeenCalledWith(nonce, session1);

    const writableNextSession = storageFrom(durable, true);
    const nextAck = vi.fn(async () => "deferred" as const);
    expect(await applyPendingBackupPlaybackRestore(async () => handoff(session2), writableNextSession, nextAck)).toBe(true);
    expect(durable.get("burrata.playback-queue.v1")).toContain("occurrence-a");
    expect(JSON.parse(durable.get(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)!)).toEqual({ nonce, sessionId: session2 });
    expect(nextAck).toHaveBeenCalledWith(nonce, session2);

    // The user edits playback after that import session. The following backend
    // process acknowledges the receipt without replaying those imported values.
    durable.set("burrata.playback-queue.v1", "user-edited queue");
    durable.set("burrata.playback-position.v1", "user-edited position");
    durable.set("burrata.playback-controls.v1", "user-edited controls");
    const laterSession = storageFrom(durable, true);
    const setItem = vi.spyOn(laterSession, "setItem");
    const laterAck = vi.fn(async (ackNonce: string, receiptSessionId: string) => {
      expect(ackNonce).toBe(nonce);
      expect(receiptSessionId).toBe(session2);
      return "acknowledged" as const;
    });
    const applied = vi.fn();
    expect(await applyPendingBackupPlaybackRestore(async () => handoff(session3), laterSession, laterAck, applied)).toBe(true);
    expect(laterAck).toHaveBeenCalledOnce();
    expect(setItem).not.toHaveBeenCalled();
    // A matching receipt belongs to an earlier session and must preserve edits.
    expect(applied).not.toHaveBeenCalled();
    expect(laterSession.memory.get("burrata.playback-queue.v1")).toBe("user-edited queue");
    expect(laterSession.memory.get("burrata.playback-position.v1")).toBe("user-edited position");
    expect(laterSession.memory.get("burrata.playback-controls.v1")).toBe("user-edited controls");
  });

  it("blocks startup when localStorage silently ignores a playback write", async () => {
    const pending = handoff();
    const storage = storageFrom(new Map([["burrata.playback-queue.v1", "old queue"]]));
    const setItem = vi.fn((key: string, value: string) => {
      if (key !== "burrata.playback-queue.v1") storage.memory.set(key, value);
    });
    const ack = vi.fn(async () => "deferred" as const);
    await expect(applyPendingBackupPlaybackRestore(async () => pending, { ...storage, setItem }, ack))
      .rejects.toThrow("could not be saved");
    expect(storage.memory.get("burrata.playback-queue.v1")).toBe("old queue");
    expect(storage.memory.has(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)).toBe(false);
    expect(ack).not.toHaveBeenCalled();
  });

  it("refetches and applies a newer handoff when an old receipt acknowledgement is stale", async () => {
    const newNonce = "b1b2c3d4-e5f6-4711-8222-334455667788";
    const durable = new Map([[BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY, JSON.stringify({ nonce, sessionId: session1 })]]);
    const storage = storageFrom(durable, true);
    const getPending = vi.fn()
      .mockResolvedValueOnce(handoff(session2))
      .mockResolvedValueOnce(handoff(session2, newNonce));
    const ack = vi.fn()
      .mockResolvedValueOnce("stale" as const)
      .mockResolvedValueOnce("deferred" as const);

    expect(await applyPendingBackupPlaybackRestore(getPending, storage, ack)).toBe(true);
    expect(getPending).toHaveBeenCalledTimes(2);
    expect(ack).toHaveBeenNthCalledWith(1, nonce, session1);
    expect(ack).toHaveBeenNthCalledWith(2, newNonce, session2);
    expect(storage.memory.get("burrata.playback-queue.v1")).toContain("occurrence-a");
    expect(JSON.parse(storage.memory.get(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)!)).toEqual({ nonce: newNonce, sessionId: session2 });
  });

  it("keeps the one-shot engine-switch controls ahead of durable values", async () => {
    const storage = storageFrom(new Map([
      ["burrata.playback-controls.v1", JSON.stringify({ volume: 0.4, shuffle: false })],
      ["burrata.engine-switch-controls.v1", JSON.stringify({ volume: 0.7, shuffle: true })],
    ]));
    const fakeStorage = { ...storage, removeItem: (key: string) => storage.memory.delete(key) } as unknown as Storage;
    expect(consumeStartupPlaybackControls(fakeStorage)).toEqual({ volume: 0.7, shuffle: true });
    expect(storage.memory.has("burrata.engine-switch-controls.v1")).toBe(false);
    const setItem = storage.setItem;
    await applyPendingBackupPlaybackRestore(async () => handoff(), { ...storage, setItem }, async () => "deferred");
    expect(consumeStartupPlaybackControls(fakeStorage)).toEqual({ volume: 0, shuffle: true });
  });

  it("does nothing without a handoff and preserves existing WebView playback state", async () => {
    const storage = storageFrom(new Map([
      ["burrata.playback-queue.v1", "ordinary queue"],
      ["burrata.playback-controls.v1", JSON.stringify({ volume: 0, shuffle: false })],
    ]));
    const set = vi.spyOn(storage, "setItem");
    const ack = vi.fn(async () => "acknowledged" as const);
    expect(await applyPendingBackupPlaybackRestore(async () => null, storage, ack)).toBe(false);
    expect(set).not.toHaveBeenCalled();
    expect(ack).not.toHaveBeenCalled();
    expect(storage.memory.get("burrata.playback-queue.v1")).toBe("ordinary queue");
    expect(readPlaybackControls({ getItem: key => storage.memory.get(key) ?? null } as Storage)).toEqual({ volume: 0, shuffle: false });
  });

  it("rejects duplicate occurrence identities before changing storage", async () => {
    const value = handoff(); value.queue.tracks = [{ id: "local:a" } as never, { id: "local:b" } as never]; value.queue.entryIds = ["same", "same"];
    const storage = storageFrom(); const set = vi.spyOn(storage, "setItem"); const ack = vi.fn(async () => "deferred" as const);
    await expect(applyPendingBackupPlaybackRestore(async () => value, storage, ack)).rejects.toThrow("Invalid pending playback restore");
    expect(set).not.toHaveBeenCalled(); expect(ack).not.toHaveBeenCalled();
  });

  it.each([2, 3, 4, 5])("retains marker when localStorage write %i throws, then retries", async (failedWrite: number) => {
    const value = handoff(); let writes = 0; const storage = storageFrom();
    const set = vi.fn((key: string, item: string) => { if (++writes === failedWrite) throw new Error("quota"); storage.setItem(key, item); });
    const ack = vi.fn(async () => "deferred" as const);
    await expect(applyPendingBackupPlaybackRestore(async () => value, { ...storage, setItem: set }, ack)).rejects.toThrow("quota");
    expect(ack).not.toHaveBeenCalled();
    expect(storage.memory.has(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY)).toBe(false);
    await expect(applyPendingBackupPlaybackRestore(async () => value, storage, ack)).resolves.toBe(true);
    expect(ack).toHaveBeenCalledWith(value.nonce, session1);
    expect(readPlaybackControls({ getItem: key => storage.memory.get(key) ?? null } as Storage)).toEqual({ volume: 0, shuffle: true });
  });

  it("keeps startup blocked when acknowledgement fails, then retries before initialization", async () => {
    const value = handoff(); const storage = storageFrom(); const initializeAppLifecycle = vi.fn(); let firstAck = true;
    const hydrate = () => applyPendingBackupPlaybackRestore(async () => value, storage, async () => {
      if (firstAck) { firstAck = false; throw new Error("stale nonce"); }
      return "deferred";
    }).then(() => true, () => false);
    expect(await startAfterBackupHydration(hydrate, initializeAppLifecycle)).toBe(false);
    expect(initializeAppLifecycle).not.toHaveBeenCalled();
    expect(await startAfterBackupHydration(hydrate, initializeAppLifecycle)).toBe(true);
    expect(initializeAppLifecycle).toHaveBeenCalledOnce();
  });

  it("accepts a paused zero-volume empty queue", async () => {
    const value = handoff(); value.queue = { tracks: [], entryIds: [], currentIndex: 0, repeat: "off" }; value.position = 0;
    const memory = new Map<string, string>();
    const set = vi.fn((key: string, item: string) => memory.set(key, item)); const ack = vi.fn(async () => "deferred" as const);
    expect(await applyPendingBackupPlaybackRestore(async () => value, { getItem: key => memory.get(key) ?? null, setItem: set }, ack)).toBe(true);
    expect(ack).toHaveBeenCalledWith(nonce, session1);
    expect(JSON.parse(set.mock.calls[0]![1])).toMatchObject({ tracks: [], entryIds: [], currentIndex: 0 });
    expect(JSON.parse(set.mock.calls[2]![1])).toEqual({ volume: 0, shuffle: true });
    const invalid = handoff(); invalid.paused = false as never;
    await expect(applyPendingBackupPlaybackRestore(async () => invalid, { getItem: () => null, setItem: set }, ack)).rejects.toThrow("Invalid pending playback restore");
  });
});
