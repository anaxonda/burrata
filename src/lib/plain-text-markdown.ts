/**
 * Normalize the small, common Markdown subset used in AI context answers for
 * display in a plain-text paragraph. This never creates markup or HTML.
 */
export function plainTextMarkdown(value: unknown): string {
  if (typeof value !== "string") return "";

  const urls: string[] = [];
  let placeholderPrefix = "\uE000burrata-url-";
  while (value.includes(placeholderPrefix)) placeholderPrefix += "x";
  const keepUrl = (url: string) => `${placeholderPrefix}${urls.push(url) - 1}\uE001`;
  let text = value.replace(/!?\[([^\]\n]*)\]\((https?:\/\/[^()\s]+)\)/gi, (_match, label: string, url: string) => `${label} (${keepUrl(url)})`);
  // URLs can contain balanced parentheses and Markdown-looking path/query
  // text. Consume through whitespace so no later formatting rule can edit it.
  text = text.replace(/https?:\/\/[^\s<>]+/gi, url => keepUrl(url));

  text = text.split(/\r?\n/).map(line => {
    let normalized = line
      .replace(/^ {0,3}#{1,6}(?=\s|$)\s?/, "")
      .replace(/^ {0,3}>\s?/, "")
      .replace(/^(\s{0,3})[-+*]\s+/, "$1• ");

    normalized = normalized
      .replace(/(?<![\w*])\*\*(?=\S)([^\n*]+?)(?<=\S)\*\*(?![\w*])/g, "$1")
      .replace(/(?<![\w_])__(?=\S)([^\n_]+?)(?<=\S)__(?![\w_])/g, "$1")
      .replace(/~~(?=\S)([^\n~]+?)(?<=\S)~~/g, "$1")
      .replace(/(?<![\w*])\*(?!\s)([^*\n]+?)(?<!\s)\*(?![\w*])/g, "$1")
      .replace(/(?<![\w])_(?=\S)([^\n_]+?)(?<=\S)_(?![\w])/g, "$1")
      .replace(/`([^`\n]+)`/g, "$1");
    return normalized;
  }).join("\n");

  const escapedPrefix = placeholderPrefix.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const token = new RegExp(`${escapedPrefix}(\\d+)\\uE001`, "g");
  return text.replace(token, (_match, index: string) => urls[Number(index)] ?? "");
}
