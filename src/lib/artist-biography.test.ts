// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { biographySegments } from "./artist-biography";

describe("artist biographies", () => {
  it("normalizes Discogs paragraphs and links from the reported profile", () => {
    expect(biographySegments("Marty\r\n\rMet [url=https://en.wikipedia.org/wiki/Little_Jimmy_Dickens]Jimmy Dickens[/url] and [a2118004].", "Discogs")).toEqual([
      { text: "Marty\n\nMet " },
      { text: "Jimmy Dickens", url: "https://en.wikipedia.org/wiki/Little_Jimmy_Dickens" },
      { text: " and " }, { text: "Artist on Discogs", url: "https://www.discogs.com/artist/2118004" }, { text: "." }
    ]);
  });
  it("never creates unsafe URLs or interprets HTML", () => {
    const input = '[url=javascript:alert(1)]click[/url] <img src=x onerror=alert(1)> [url=https://evil.example@host.example/]bad[/url]';
    expect(biographySegments(input, "Discogs")).toEqual([{ text: 'click <img src=x onerror=alert(1)> bad' }]);
  });
  it("keeps malformed markup as text and recognizes named and formatted credits", () => {
    expect(biographySegments("[a=Jimmy Dickens] [b]bold[/b] [url=https://x.test]unfinished", "Discogs")).toEqual([
      {text:"Jimmy Dickens",url:"https://www.discogs.com/search/?q=Jimmy+Dickens&type=artist"}, {text:" bold [url=https://x.test]unfinished"}
    ]);
  });
  it.each(["Wikipedia", "MusicBrainz", "Spotify"])("treats %s text as plain text", source => {
    expect(biographySegments("[a123]\rFirst\r\n\r\nSecond <b>literal</b>", source)).toEqual([{text:"[a123]\nFirst\n\nSecond <b>literal</b>"}]);
  });
  it("normalizes visible carriage-return glyphs and renders nested Discogs formatting", () => {
    expect(biographySegments("Marty␍␍Met [url=https://example.org][b]Jimmy[/b] Dickens[/url] and [a2118004].", "discogs")).toEqual([
      {text:"Marty\n\nMet "}, {text:"Jimmy Dickens",url:"https://example.org/"}, {text:" and "}, {text:"Artist on Discogs",url:"https://www.discogs.com/artist/2118004"}, {text:"."}
    ]);
  });
  it("renders Monochrome HTML as safe text and links while preserving paragraphs", () => {
    const segments = biographySegments('<p>First &amp; <a href="https://example.org/a"><b>artist</b></a>.</p><p>Second<br>line <a href="javascript:alert(1)">unsafe</a><script>bad()</script></p>', "Monochrome");
    expect(segments).toEqual([{text:"First & "},{text:"artist",url:"https://example.org/a"},{text:".\n\nSecond\nline unsafe"}]);
  });
  it("does not interpret markup-like plain Wikipedia or MusicBrainz text", () => {
    for (const source of ["Wikipedia", "MusicBrainz"]) expect(biographySegments("[b]Literal[/b] <p>text</p>", source)).toEqual([{text:"[b]Literal[/b] <p>text</p>"}]);
  });

  it("decodes cached Last.fm entities and safely renders raw biography paragraphs", () => {
    expect(biographySegments("One &amp; Two &#39;three&#39;", "Last.fm")).toEqual([{text:"One & Two 'three'"}]);
    expect(biographySegments('<p>First &amp; more</p><p>Second <a href="https://example.org">link</a><script>bad()</script></p>', "Last.fm")).toEqual([
      {text:"First & more\n\nSecond "},{text:"link",url:"https://example.org/"}
    ]);
  });

});
