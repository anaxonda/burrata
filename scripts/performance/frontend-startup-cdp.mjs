import fs from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

const [distDir, label = "sample", countValue = "40", outputPath] = process.argv.slice(2);
if (!distDir) throw new Error("usage: node /tmp/burrata-startup-measure.mjs <dist-dir> <label> [measured-attempts]");
const root = path.resolve(distDir);
const count = Number(countValue);
if (!Number.isInteger(count) || count < 2 || count % 2 !== 0) throw new Error("measured-attempts must be an even integer of at least 2 (half cold, half warm)");
const profile = `/tmp/burrata-startup-profile-${label}`;
await fs.rm(profile, { recursive: true, force: true });

const mime = new Map([[".html", "text/html"], [".js", "text/javascript"], [".css", "text/css"], [".png", "image/png"], [".svg", "image/svg+xml"], [".woff2", "font/woff2"]]);
const server = http.createServer(async (request, response) => {
  try {
    const urlPath = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const candidate = path.resolve(root, `.${urlPath === "/" ? "/index.html" : urlPath}`);
    if (!candidate.startsWith(root + path.sep) && candidate !== path.join(root, "index.html")) throw new Error("path outside dist");
    const data = await fs.readFile(candidate);
    response.writeHead(200, {
      "content-type": mime.get(path.extname(candidate)) ?? "application/octet-stream",
      "cache-control": path.extname(candidate) === ".html" ? "no-cache" : "public, max-age=31536000, immutable",
    });
    response.end(data);
  } catch {
    response.writeHead(404); response.end("not found");
  }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const serverPort = server.address().port;
const chrome = spawn("chromium", [
  "--headless", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage",
  "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank",
], { stdio: "ignore" });

class CDP {
  nextId = 0;
  pending = new Map();
  constructor(socket) {
    this.socket = socket;
    socket.addEventListener("message", event => {
      const message = JSON.parse(event.data);
      if (!message.id) return;
      const entry = this.pending.get(message.id);
      if (!entry) return;
      this.pending.delete(message.id);
      if (message.error) entry.reject(new Error(message.error.message));
      else entry.resolve(message.result);
    });
  }
  call(method, params = {}) {
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async evaluate(expression) {
    const result = await this.call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
    return result.result?.value;
  }
}

async function waitForEndpoint() {
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const active = (await fs.readFile(path.join(profile, "DevToolsActivePort"), "utf8")).split("\n");
      const port = Number(active[0]);
      const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = pages.find(entry => entry.type === "page");
      if (page?.webSocketDebuggerUrl) return { port, page };
    } catch { /* chromium is starting */ }
    await delay(100);
  }
  throw new Error("Chromium remote debugging endpoint did not start");
}
const { port, page } = await waitForEndpoint();
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { socket.addEventListener("open", resolve, { once: true }); socket.addEventListener("error", reject, { once: true }); });
const cdp = new CDP(socket);
await cdp.call("Page.enable");
await cdp.call("Runtime.enable");
await cdp.call("Network.enable");
await cdp.call("Page.addScriptToEvaluateOnNewDocument", { source: `(() => {
  const m = window.__startupMeasure = { started: performance.now() };
  const scan = () => {
    const shell = document.querySelector('.app-shell');
    if (shell && m.shellMs == null) m.shellMs = performance.now();
    const controls = document.querySelector('#burrata-playback-controls');
    const play = document.querySelector('button.play-button');
    if (controls && play && m.controlsMs == null) {
      m.controlsMs = performance.now();
      const interactionStarted = performance.now();
      play.focus({ preventScroll: true });
      play.click();
      m.firstInteractionMs = performance.now();
      m.firstInteractionLatencyMs = m.firstInteractionMs - interactionStarted;
      m.complete = true;
      observer.disconnect();
    }
  };
  const observer = new MutationObserver(scan);
  observer.observe(document, { childList: true, subtree: true });
  document.addEventListener('DOMContentLoaded', () => { m.domContentLoadedMs = performance.now(); scan(); }, { once: true });
  window.addEventListener('load', () => { m.loadMs = performance.now(); scan(); }, { once: true });
})();` });
const baseUrl = `http://127.0.0.1:${serverPort}/`;
let navigation = 0;

async function navigate({ cold = false, initial = false } = {}) {
  if (cold) await cdp.call("Network.clearBrowserCache");
  const url = `${baseUrl}?measure=${++navigation}`;
  await cdp.call("Page.navigate", { url });
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const state = await cdp.evaluate(`window.__startupMeasure && JSON.stringify(window.__startupMeasure)`);
    if (state) {
      const parsed = JSON.parse(state);
      if (parsed.complete) return parsed;
    }
    await delay(10);
  }
  const state = await cdp.evaluate(`window.__startupMeasure && JSON.stringify(window.__startupMeasure)`);
  throw new Error(`page did not reach core controls (${initial ? "warmup" : "measurement"}): ${state}`);
}

try {
  await navigate({ initial: true }); // three warmups excluded from the sample
  await navigate({ initial: true });
  await navigate({ initial: true });
  const samples = [];
  for (let index = 0; index < count; index++) {
    const cold = index % 2 === 0;
    const marks = await navigate({ cold });
    samples.push({ attempt: index + 1, cache: cold ? "cold" : "warm", ...marks });
  }
  const summary = Object.fromEntries(["cold", "warm"].map(cache => {
    const values = samples.filter(sample => sample.cache === cache).map(sample => sample.controlsMs);
    values.sort((a, b) => a - b);
    const at = p => values[Math.min(values.length - 1, Math.ceil(p * values.length) - 1)];
    return [cache, { n: values.length, p50Ms: at(.5), p95Ms: at(.95) }];
  }));
  const output = JSON.stringify({
    label,
    root,
    browser: "Chromium headless; isolated profile per label",
    warmupsExcluded: 3,
    measuredAttempts: count,
    scope: "Production App DOM mount to app shell and #burrata-playback-controls/.play-button availability, then synthetic click dispatch return. Not native playback readiness.",
    summary,
    samples,
  }, null, 2) + "\n";
  if (outputPath) await fs.writeFile(outputPath, output);
  process.stdout.write(output);
} finally {
  socket.close();
  chrome.kill("SIGTERM");
  await new Promise(resolve => server.close(resolve));
}
