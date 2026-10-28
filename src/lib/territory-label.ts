// Presentation only: retain source country codes in stored catalog metadata.
let regions: Intl.DisplayNames | undefined;
try { regions = new Intl.DisplayNames(["en"], { type: "region" }); } catch { /* Older WebViews retain the original code. */ }
export function territoryLabel(value?: string | null): string {
  const raw = value?.trim() ?? "";
  const code = raw.toUpperCase();
  if (code === "XW") return "Worldwide";
  if (code === "XE") return "Europe";
  if (!/^[A-Z]{2}$/.test(code)) return raw;
  try { return regions?.of(code) ?? raw; } catch { return raw; }
}
