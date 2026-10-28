export type RestoreJobProgress = {
  jobId?: string;
  state: "queued" | "running" | "complete" | "failed" | "canceled";
  phase?: string;
  message?: string;
};

/** Wait until a restore worker finishes before telling the UI it is safe to restart. */
export async function startRestoreAndWaitForCompletion(
  start: () => Promise<{ jobId: string }>,
  waitForJob: (jobId: string, report: (progress: RestoreJobProgress) => void) => Promise<unknown>,
  report: (progress: RestoreJobProgress) => void,
): Promise<{ jobId: string; restartRequired: true }> {
  const { jobId } = await start();
  report({ jobId, state: "running", phase: "Preparing restore", message: "Preparing restore before restart." });
  await waitForJob(jobId, report);
  return { jobId, restartRequired: true };
}
