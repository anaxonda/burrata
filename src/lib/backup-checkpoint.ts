export type BackupCheckpointRequest = {
  requestId: string;
  includeWebviewPlayback: boolean;
  restorePurpose?: boolean;
  deadlineMs: number;
};

export type BackupCheckpointResponse = {
  requestId: string;
  frontendPreferences: Record<string, unknown>;
  webviewPlayback?: Record<string, unknown>;
};

function validateBackupCheckpointRequest(request: BackupCheckpointRequest): void {
  if (typeof request.requestId !== "string" || request.requestId.length < 1 || request.requestId.length > 128
    || /[\u0000-\u001f\u007f]/.test(request.requestId)) {
    throw new Error("Invalid backup checkpoint request ID");
  }
  if (!Number.isFinite(request.deadlineMs) || request.deadlineMs <= 0) {
    throw new Error("Invalid backup checkpoint deadline");
  }
}

/** Build the response to one bounded native checkpoint request. */
export function createBackupCheckpointResponse(
  request: BackupCheckpointRequest,
  frontendPreferences: Record<string, unknown>,
  webviewPlayback?: Record<string, unknown>,
): BackupCheckpointResponse {
  validateBackupCheckpointRequest(request);
  if (request.includeWebviewPlayback && !webviewPlayback) {
    throw new Error("The requested WebView playback checkpoint is unavailable");
  }
  return {
    requestId: request.requestId,
    frontendPreferences,
    ...(request.includeWebviewPlayback ? { webviewPlayback } : {}),
  };
}

/** Capture and submit one response to the native backup coordinator request. */
export async function respondToBackupCheckpoint(
  request: BackupCheckpointRequest,
  captureFrontendPreferences: () => Record<string, unknown>,
  captureWebviewPlayback: () => Record<string, unknown>,
  submit: (response: BackupCheckpointResponse) => Promise<unknown>,
  pauseForRestorePurpose?: () => Promise<void>,
): Promise<BackupCheckpointResponse> {
  // Validate before pausing: malformed native events must not interrupt playback.
  validateBackupCheckpointRequest(request);
  if (request.restorePurpose) {
    if (!pauseForRestorePurpose) {
      throw new Error("Restore checkpoint playback pause is unavailable");
    }
    await pauseForRestorePurpose();
  }
  const response = createBackupCheckpointResponse(
    request,
    captureFrontendPreferences(),
    request.includeWebviewPlayback ? captureWebviewPlayback() : undefined,
  );
  await submit(response);
  return response;
}
