export type BiographySegment = { text: string; url?: string };

function safeUrl(value: string): string | undefined {
  try {
    const url = new URL(value.trim());
    return url.protocol === "https:" && url.hostname && !url.username && !url.password ? url.href : undefined;
  } catch { return undefined; }
}

function push(result: BiographySegment[], text: string, url?: string) {
  if (!text) return;
  const last = result[result.length - 1];
  if (last && last.url === url) last.text += text;
  else result.push({text, ...(url ? {url} : {})});
}

/** Render provider text as text and explicitly validated links, never HTML. */
export function biographySegments(text: string, source?: string | null): BiographySegment[] {
  const normalized = text.replace(/\r\n?|\u240d\n?/g, "\n");
  const provider = source?.trim().toLowerCase();
  if (provider === "discogs") return discogsSegments(normalized);
  if ((provider === "monochrome" || provider === "last.fm") && /<\/?(?:p|br|a|b|strong|i|em|div|span|ul|ol|li|h[1-6]|script|style)\b|&(?:amp|lt|gt|quot|apos|nbsp|#\d+|#x[\da-f]+);/i.test(normalized) && typeof document !== "undefined") return htmlSegments(normalized);
  return normalized ? [{text:normalized}] : [];
}

function discogsSegments(text: string): BiographySegment[] {
  const result: BiographySegment[] = [];
  const tokens = /\[url(?:=([^\]\r\n]+))?\]([\s\S]*?)\[\/url\]|\[([arml])(\d+)\]|\[([arml])=([^\]\r\n]+)\]|\[\/?(?:b|i|u|s|quote)\]/gi;
  const entities: Record<string, [string, string]> = {a:["artist","Artist on Discogs"],r:["release","Release on Discogs"],m:["master","Release on Discogs"],l:["label","Label on Discogs"]};
  let position = 0;
  for (const match of text.matchAll(tokens)) {
    push(result, text.slice(position, match.index));
    if (match[2] !== undefined) {
      // Nested formatting is rendered as text inside a single safe link.
      const label = discogsSegments(match[2]).map(segment => segment.text).join("");
      push(result, label, safeUrl(match[1] ?? label));
    } else if (match[3]) {
      const [path, label] = entities[match[3].toLowerCase()];
      push(result, label, `https://www.discogs.com/${path}/${match[4]}`);
    } else if (match[5]) {
      const name = match[6].trim(), kind = entities[match[5].toLowerCase()][0];
      if (name) {
        const search = new URL("https://www.discogs.com/search/");
        search.searchParams.set("q", name); search.searchParams.set("type", kind);
        push(result, match[6], search.href);
      } else push(result, match[0]);
    }
    position = match.index + match[0].length;
  }
  push(result, text.slice(position));
  return result;
}

function htmlSegments(text: string): BiographySegment[] {
  const template = document.createElement("template"), result: BiographySegment[] = [];
  template.innerHTML = text;
  const newline = () => { if (result.length && !result[result.length - 1]!.text.endsWith("\n")) push(result, "\n"); };
  const walk = (node: Node, link?: string) => {
    if (node.nodeType === 3) { push(result, node.textContent ?? "", link); return; }
    if (node.nodeType !== 1) return;
    const element = node as Element, tag = element.tagName.toLowerCase();
    if (["script", "style", "iframe", "object", "template"].includes(tag)) return;
    if (tag === "br") { push(result, "\n"); return; }
    const block = ["p", "div", "li", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6"].includes(tag);
    if (block) newline();
    const href = tag === "a" ? safeUrl(element.getAttribute("href") ?? "") : link;
    for (const child of element.childNodes) walk(child, href);
    if (block) { newline(); if (tag === "p") push(result, "\n"); }
  };
  for (const child of template.content.childNodes) walk(child);
  while (result.length && /^\n+$/.test(result[result.length - 1]!.text)) result.pop();
  if (result.length) result[result.length - 1].text = result[result.length - 1]!.text.replace(/\n+$/, "");
  return result;
}
