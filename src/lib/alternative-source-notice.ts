export type AlternativeSourceNotice = {label:string; performer:string | null; requested:string; playing:string};
/** Only use structured, accepted matching evidence; never infer performer from an uploader. */
export function alternativeSourceNotice(value: unknown, requestedTitle?: string | null, requestedArtist?: string | null): AlternativeSourceNotice | null {
  if (!value || typeof value !== "object") return null;
  const match = value as Record<string, unknown>;
  if (!match.assessment || typeof match.assessment !== "object" || !match.resolvedRecording || typeof match.resolvedRecording !== "object") return null;
  const assessment = match.assessment as Record<string, unknown>, resolved = match.resolvedRecording as Record<string, unknown>;
  if (assessment.relationship !== "alternative" || assessment.action !== "accept") return null;
  const labels: Record<string,string> = {release:"Different release",performance:"Other performance",remix:"Other remix or edit",otherArtist:"Other artist"};
  const kind = typeof assessment.alternativeKind === "string" ? assessment.alternativeKind : "";
  const clean = (value:unknown) => typeof value === "string" && value.trim() ? value.trim().slice(0, 200) : null;
  const title = clean(resolved.title) ?? "Alternative recording";
  const performer = clean(resolved.verifiedPerformer);
  const requested = [clean(requestedTitle),clean(requestedArtist)].filter(Boolean).join(" · ") || "the selected recording";
  return {label:labels[kind] ?? "Alternative recording", performer, requested, playing:[title,performer].filter(Boolean).join(" · ")};
}
