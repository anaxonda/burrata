import { describe, expect, it, vi } from "vitest";
import { startRestoreAndWaitForCompletion, type RestoreJobProgress } from "./restore-job";

describe("startRestoreAndWaitForCompletion", () => {
  it("does not report a staged restart until the restore job completes", async () => {
    let finish!: () => void;
    const start = vi.fn().mockResolvedValue({ jobId: "restore-1" });
    const waitForJob = vi.fn((_jobId: string, _report: (progress: RestoreJobProgress) => void) => new Promise<void>(resolve => { finish = resolve; }));
    const report = vi.fn();
    let resolved = false;

    const pending = startRestoreAndWaitForCompletion(start, waitForJob, report).then(result => {
      resolved = true;
      return result;
    });
    await Promise.resolve();

    expect(start).toHaveBeenCalledTimes(1);
    expect(waitForJob).toHaveBeenCalledWith("restore-1", report);
    expect(report).toHaveBeenCalledWith({
      jobId: "restore-1", state: "running", phase: "Preparing restore", message: "Preparing restore before restart.",
    });
    expect(resolved).toBe(false);

    finish();
    await expect(pending).resolves.toEqual({ jobId: "restore-1", restartRequired: true });
  });

  it("propagates worker failure and never returns a restart-required result", async () => {
    const report = vi.fn();
    const waitForJob = vi.fn().mockRejectedValue(new Error("Disk quota exceeded"));

    await expect(startRestoreAndWaitForCompletion(
      async () => ({ jobId: "restore-2" }), waitForJob, report,
    )).rejects.toThrow("Disk quota exceeded");
    expect(waitForJob).toHaveBeenCalledWith("restore-2", report);
  });
});
