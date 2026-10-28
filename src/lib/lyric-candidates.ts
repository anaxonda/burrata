export type LyricResultSort = "match" | "duration" | "shortest" | "longest";
type Result = { id: string; durationSeconds: number; automaticMatch: boolean; plainTextMatch?: boolean };
export function sortLyricCandidates<T extends Result>(rows: T[], duration: number | null | undefined, sort: LyricResultSort): T[] {
  if (sort === "match") return [...rows]; // Provider ranks verified matches first.
  const valid = (n: number | null | undefined): n is number => typeof n === "number" && Number.isFinite(n) && n > 0;
  const score = (row: T) => !valid(row.durationSeconds) ? Infinity : sort === "duration" ? valid(duration) ? Math.abs(row.durationSeconds - duration) : Infinity : sort === "longest" ? -row.durationSeconds : row.durationSeconds;
  return rows.map((row,index)=>({row,index})).sort((a,b)=>score(a.row)-score(b.row)||a.index-b.index).map(item=>item.row);
}
export function lyricRuntime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "Unknown duration";
  const whole = Math.round(seconds);
  return `${Math.floor(whole/60)}:${String(whole%60).padStart(2,"0")}`;
}
export function lyricDurationDifference(seconds: number, current: number | null | undefined): string {
  if (typeof current !== "number" || !Number.isFinite(current) || current <= 0 || !Number.isFinite(seconds)) return "";
  const delta = Math.round(seconds - current);
  return delta === 0 ? "Same duration" : `${lyricRuntime(Math.abs(delta))} ${delta < 0 ? "shorter" : "longer"}`;
}
