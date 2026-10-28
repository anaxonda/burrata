import { describe, expect, it, vi } from "vitest";
import { startAfterBackupHydration } from "./backup-startup-gate";

describe("backup hydration startup gate", () => {
  it("does not initialize, publish, or start background work when persistence fails, and permits a successful retry", async () => {
    const initialize = vi.fn();
    const hydrate = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    expect(await startAfterBackupHydration(hydrate, initialize)).toBe(false);
    expect(initialize).not.toHaveBeenCalled();
    expect(await startAfterBackupHydration(hydrate, initialize)).toBe(true);
    expect(initialize).toHaveBeenCalledOnce();
  });
});
