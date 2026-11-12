import { expect, it } from "vitest";
import { plainTextMarkdown } from "./plain-text-markdown";

it("removes common balanced Markdown while keeping words, headings, lists, and citation URLs", () => {
  const answer = [
    "## Event date",
    "**27 February 2026** — *Fred again* and _Thomas Bangalter_ played `USB002`.",
    "- [Official event page](https://events.example.org/usb002?id=27)",
    "> Source-backed summary",
  ].join("\n");

  expect(plainTextMarkdown(answer)).toBe([
    "Event date",
    "27 February 2026 — Fred again and Thomas Bangalter played USB002.",
    "• Official event page (https://events.example.org/usb002?id=27)",
    "Source-backed summary",
  ].join("\n"));
});

it("leaves unmatched formatting markers and plain URLs intact", () => {
  expect(plainTextMarkdown("An **unfinished marker and https://example.org/a_b. stays literal."))
    .toBe("An **unfinished marker and https://example.org/a_b. stays literal.");
});

it("does not treat asterisks inside artist names as emphasis delimiters", () => {
  expect(plainTextMarkdown("A*Teens and B*Witched, plus *a real emphasis span*."))
    .toBe("A*Teens and B*Witched, plus a real emphasis span.");
});

it("protects complete parenthetical URLs containing Markdown-like text", () => {
  const bare = "https://music.example.org/release_(**edition**)/?name=__value__";
  const complexLink = "[archive](https://music.example.org/release_(**edition**)/?name=__value__)";
  expect(plainTextMarkdown(`${bare}\n${complexLink}`)).toBe(`${bare}\n${complexLink}`);
});

it("does not reinterpret text that resembles an internal URL placeholder", () => {
  const marker = "\uE000burrata-url-0\uE001";
  expect(plainTextMarkdown(`${marker} **visible**`)).toBe(`${marker} visible`);
});

it("treats HTML-looking content as plain text and ignores non-string values", () => {
  expect(plainTextMarkdown("<img src=x onerror=alert(1)> **Visible text**"))
    .toBe("<img src=x onerror=alert(1)> Visible text");
  expect(plainTextMarkdown({ answer: "not text" })).toBe("");
});
