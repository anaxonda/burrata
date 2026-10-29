import type { DownloadJob } from "./types";

const states: Record<string, string> = {
  completed: "Saved to library", failed: "Could not download", cancelled: "Cancelled",
  interrupted: "Interrupted", waiting_for_peer: "Waiting for peer", waiting_for_source: "Waiting for source", waiting_for_playback: "Upgrade ready · waiting for playback", queued: "In queue",
  identifying: "Identifying audio", downloading: "Downloading", validating: "Checking audio", tagging: "Writing tags",
  stalled: "No recent progress",
  publishing: "Saving file", finalizing: "Finishing",
};
const sources: Record<string, string> = {
  slskd: "Soulseek", youtube: "YouTube", soundcloud: "SoundCloud", direct: "Direct download",
  monochrome: "Monochrome",
};
export const downloadStateLabel = (state: string) => states[state] || state;
export const downloadSourceLabel = (job: DownloadJob) => sources[job.candidate.provider] || job.candidate.provider;
export const downloadPeer = (job: DownloadJob) => job.candidate.source?.username || job.candidate.uploader;
export const downloadTitle = (job: DownloadJob) => job.track.title || job.candidate.title.split(/[\\/]/).pop() || "Untitled recording";
export function downloadTransferDetail(job: DownloadJob): string {
  const info = job.soulseek;
  if (!info) return "";
  if (info.nextAttemptAt) return `Source retry scheduled for ${new Date(info.nextAttemptAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
  if (job.state === "waiting_for_peer") {
    if (info.queueError) return "Source has not answered the queue check";
    if (info.queuePosition != null) return `Position ${info.queuePosition.toLocaleString()} in source queue`;
    return "Queue position not available";
  }
  if (job.state === "stalled") {
    return `${(info.bytesReceived / 1048576).toFixed(1)} MB received${info.lastProgressAt ? ` · last data at ${new Date(info.lastProgressAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : ""}`;
  }
  if (job.state === "downloading" && info.bytesReceived > 0) {
    return `${(info.bytesReceived / 1048576).toFixed(1)} MB received${info.speedBytesPerSecond > 0 ? ` · ${(info.speedBytesPerSecond / 1024).toFixed(0)} KB/s` : ""}`;
  }
  return "";
}
