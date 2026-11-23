import type { BrowserStatus } from "./types";

export function browserCompanionStatus(status: BrowserStatus | null): string {
  if (!status?.connected) return "Browser companion not detected. Open the extension popup to connect.";
  if ((status.providerProtocolVersion ?? 0) >= 2) {
    return "Browser companion connected · Browser-assisted access on";
  }
  return status.providerProtocolVersion === 0
    ? "Browser companion connected · Browser-assisted access off"
    : "Browser companion connected";
}
