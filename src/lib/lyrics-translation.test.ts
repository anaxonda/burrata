import { describe, expect, it } from "vitest";
import { validateLyricsTranslation, type LyricsSourceLine } from "./lyrics-translation";

const source: LyricsSourceLine[] = [
  { id: "line-a", timeSeconds: 1.25, text: "Synthetic original A" },
  { id: "line-b", timeSeconds: 2.5, text: "Synthetic original B" },
];

describe("validateLyricsTranslation", () => {
  it("accepts complete ordered translations for the unchanged source hash", () => {
    const result = { sourceHash: "source-v1", targetLanguage: "es", lines: [
      { id: "line-a", text: "Traducción A" }, { id: "line-b", text: "Traducción B" },
    ] };
    expect(validateLyricsTranslation("source-v1", source, result)).toBe(result);
  });

  it("rejects stale sources, missing lines, reordered IDs, and incomplete translations", () => {
    const valid = { sourceHash: "source-v1", targetLanguage: "es", lines: [
      { id: "line-a", text: "A" }, { id: "line-b", text: "B" },
    ] };
    expect(validateLyricsTranslation("source-v2", source, valid)).toBeNull();
    expect(validateLyricsTranslation("source-v1", source, { ...valid, lines: valid.lines.slice(0, 1) })).toBeNull();
    expect(validateLyricsTranslation("source-v1", source, { ...valid, lines: [...valid.lines].reverse() })).toBeNull();
    expect(validateLyricsTranslation("source-v1", source, { ...valid, lines: valid.lines.map(line => ({ ...line, id: "wrong" })) })).toBeNull();
  });

  it("preserves an empty paragraph and rejects blanking nonempty source text", () => {
    const paragraphs: LyricsSourceLine[] = [
      { id: "p1", timeSeconds: null, text: "Synthetic paragraph" },
      { id: "p2", timeSeconds: null, text: "" },
    ];
    expect(validateLyricsTranslation("plain-v1", paragraphs, { sourceHash: "plain-v1", targetLanguage: "es", lines: [
      { id: "p1", text: "Párrafo sintético" }, { id: "p2", text: "" },
    ] })).not.toBeNull();
    expect(validateLyricsTranslation("plain-v1", paragraphs, { sourceHash: "plain-v1", targetLanguage: "es", lines: [
      { id: "p1", text: "" }, { id: "p2", text: "" },
    ] })).toBeNull();
  });
});
