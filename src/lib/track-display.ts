import type { Track } from "./types";

function titleKey(value: string): string {
  return value.normalize("NFKC").toLowerCase().replace(/[^\p{L}\p{M}\p{N}]+/gu, " ").trim();
}

export function trackDisplayVersion(track: Track): string {
  const version = track.version?.trim() ?? "";
  const manual = (track.provenance as { manualFields?: { version?: boolean } } | null)?.manualFields;
  if (!version || manual?.version) return version;

  const titleSuffix = /\(([^()]*)\)$|\[([^\[\]]*)\]$|\s[-–—]\s(.+)$/.exec(track.title.trim());
  const suffix = titleSuffix?.[1] ?? titleSuffix?.[2] ?? titleSuffix?.[3];
  if (suffix && titleKey(suffix) === titleKey(version)) return "";
  if (track.provider !== "traxsource") return version;

  const clock = /(?:^|\s)\((\d{1,3}):(\d{2})\)$/.exec(version);
  const runtime = clock ? Number(clock[1]) * 60 + Number(clock[2]) : 0;
  const label = clock && runtime > 0 && Math.abs((track.durationSeconds ?? 0) - runtime) < 0.5
    ? version.slice(0, clock.index).trimEnd()
    : version;
  const repeatedTitle = /^(.*?)\s+\[([^\[\]]+)\]$/.exec(label);
  if (repeatedTitle && titleKey(repeatedTitle[1]) === titleKey(track.title.replace(/\s+\(remix\)$/i, ""))) {
    return repeatedTitle[2].trim();
  }
  return label;
}
