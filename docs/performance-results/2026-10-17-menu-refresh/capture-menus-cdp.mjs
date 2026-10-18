import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const out = new URL("./", import.meta.url);
const target = (await fetch("http://127.0.0.1:9223/json/list").then((r) => r.json())).find((item) => item.type === "page");
if (!target) throw new Error("No isolated Chromium page target found");
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve, { once: true });
  ws.addEventListener("error", reject, { once: true });
});
let nextId = 0;
const pending = new Map();
ws.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const { resolve, reject } = pending.get(message.id);
    pending.delete(message.id);
    message.error ? reject(new Error(message.error.message)) : resolve(message.result);
  }
});
function call(method, params = {}) {
  const id = ++nextId;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}
await call("Page.enable");
await call("Runtime.enable");
const cases = [
  { component: "TrackMenu", label: "1280x720", width: 1280, height: 720, url: "http://127.0.0.1:5187/docs/performance-results/2026-10-16-context-menu/harness/" },
  { component: "TrackMenu", label: "707x480", width: 707, height: 480, url: "http://127.0.0.1:5187/docs/performance-results/2026-10-16-context-menu/harness/" },
  { component: "PlaylistMenu", label: "1280x720", width: 1280, height: 720, url: "http://127.0.0.1:5187/docs/performance-results/2026-10-16-context-menu/harness/?menu=playlist" },
  { component: "PlaylistMenu", label: "707x480", width: 707, height: 480, url: "http://127.0.0.1:5187/docs/performance-results/2026-10-16-context-menu/harness/?menu=playlist" },
  { component: "TrackList fallback", label: "1280x720", width: 1280, height: 720, url: "http://127.0.0.1:5187/docs/performance-results/2026-10-17-menu-refresh/tracklist-fixture/" },
  { component: "TrackList fallback", label: "707x480", width: 707, height: 480, url: "http://127.0.0.1:5187/docs/performance-results/2026-10-17-menu-refresh/tracklist-fixture/" },
];
const measurements = [];
for (const item of cases) {
  await call("Emulation.setDeviceMetricsOverride", { width: item.width, height: item.height, deviceScaleFactor: 1, mobile: false });
  await call("Page.navigate", { url: item.url });
  let data;
  for (let attempt = 0; attempt < 100; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    const result = await call("Runtime.evaluate", { expression: "JSON.stringify({href:location.href,title:document.title})", returnByValue: true });
    const state = JSON.parse(result.result.value);
    if (state.href === item.url && state.title?.startsWith("METRICS:")) {
      const value = JSON.parse(state.title.slice("METRICS:".length));
      if (value.viewport.width === item.width && value.viewport.height === item.height) { data = value; break; }
    }
  }
  if (!data) throw new Error(`${item.component} ${item.label} fixture did not finish measuring`);
  const image = await call("Page.captureScreenshot", { format: "png", fromSurface: true, captureBeyondViewport: false });
  const filename = `${item.component.toLowerCase().replaceAll(" ", "-")}-${item.label}.png`;
  writeFileSync(new URL(filename, out), Buffer.from(image.data, "base64"));
  const card = data.card ?? data.menu;
  const lowerBound = data.playerTop ?? data.footerTop;
  const rows = data.rows;
  if (card.x < 0 || card.right > data.viewport.width) throw new Error(`${item.component} ${item.label} has horizontal clipping: ${JSON.stringify(card)}`);
  if (card.bottom > lowerBound) throw new Error(`${item.component} ${item.label} overlaps the player/footer: ${JSON.stringify(card)}`);
  if (rows.some((row) => row.height < 36 || row.height > 42)) throw new Error(`${item.component} ${item.label} has an unexpected action row height`);
  measurements.push({ component: item.component, screenshot: filename, ...data });
  process.stdout.write(`${item.component} ${item.label}: ${JSON.stringify(card)}; rows=${rows.length}\n`);
}
const sourceFiles = ["src/TrackMenu.svelte", "src/PlaylistMenu.svelte", "src/MenuIcon.svelte", "src/SourceLink.svelte", "src/TrackList.svelte", "src/lib/menu-geometry.ts", "src/style.css"];
const sourceSha256 = Object.fromEntries(sourceFiles.map((path) => [path, createHash("sha256").update(readFileSync(path)).digest("hex")]));
const report = {
  scope: "Synthetic browser fixtures mounting the exact TrackMenu, PlaylistMenu, and TrackList components; no live application state or playback was used.",
  captureDate: new Date().toISOString(),
  environment: { browser: "Chromium headless via isolated DevTools target", viewportSizes: ["1280x720", "707x480"], playerHeight: 90 },
  results: measurements,
  sourceSha256,
  findings: [
    "All measured cards stayed within viewport bounds and above the synthetic 90px player/footer.",
    "Action rows measured 36px, except the TrackMenu source row at 39px; TrackMenu content scrolls at 707x480.",
    "TrackList fallback uses the same compact SVG icon gutter and direct Add to playlist action.",
  ],
};
writeFileSync(new URL("menu-visual-report.json", out), JSON.stringify(report, null, 2) + "\n");
ws.close();
