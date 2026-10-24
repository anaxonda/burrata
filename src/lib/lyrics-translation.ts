export interface LyricsSourceLine {
  id: string;
  timeSeconds: number | null;
  text: string;
}

export interface LyricsTranslationResult {
  sourceHash: string;
  targetLanguage: string;
  provider?: string;
  model?: string;
  fallbackUsed?: boolean;
  lines: { id: string; text: string }[];
}

/** Accept only a complete, ordered translation for the exact loaded source. */
export function validateLyricsTranslation(
  sourceHash: string,
  sourceLines: readonly LyricsSourceLine[],
  result: LyricsTranslationResult | null | undefined,
): LyricsTranslationResult | null {
  if (!result || !sourceHash || result.sourceHash !== sourceHash || !result.targetLanguage?.trim()) return null;
  if (!Array.isArray(result.lines) || result.lines.length !== sourceLines.length) return null;
  for (let index = 0; index < sourceLines.length; index++) {
    const source = sourceLines[index], translated = result.lines[index];
    if (!source?.id || translated?.id !== source.id || typeof translated.text !== "string") return null;
    if (source.text.trim() && !translated.text.trim()) return null;
    if (!source.text.trim() && translated.text.trim()) return null;
  }
  return result;
}
