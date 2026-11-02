import fs from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

const [baselineValue, finalValue, outputPath] = process.argv.slice(2);
if (!baselineValue || !finalValue || !outputPath) {
  throw new Error("usage: node frontend-startup-paired-trace-cdp.mjs <baseline-dist> <final-dist> <output-json>");
}
const roots = { baseline: path.resolve(baselineValue), final: path.resolve(finalValue) };
const mime = new Map([[".html", "text/html"], [".js", "text/javascript"], [".css", "text/css"], [".png", "image/png"], [".svg", "image/svg+xml"], [".woff2", "font/woff2"]]);

function serve(root) {
  const server = http.createServer(async (request, response) => {
    try {
      const urlPath = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
      const candidate = path.resolve(root, `.${urlPath === "/" ? "/index.html" : urlPath}`);
      if (!candidate.startsWith(root + path.sep) && candidate !== path.join(root, "index.html")) throw new Error("path outside dist");
      response.writeHead(200, {
        "content-type": mime.get(path.extname(candidate)) ?? "application/octet-stream",
        "cache-control": path.extname(candidate) === ".html" ? "no-cache" : "public, max-age=31536000, immutable",
      });
      response.end(await fs.readFile(candidate));
    } catch {
      response.writeHead(404); response.end("not found");
    }
  });
  return new Promise(resolve => server.listen(0, "127.0.0.1", () => resolve({ server, port: server.address().port })));
}

class CDP {
  nextId = 0;
  pending = new Map();
  traceEvents = [];
  tracingComplete = null;
  constructor(socket) {
    this.socket = socket;
    socket.addEventListener("message", event => {
      const message = JSON.parse(event.data);
      if (message.method === "Tracing.dataCollected") this.traceEvents.push(...message.params.value);
      if (message.method === "Tracing.tracingComplete") this.tracingComplete?.();
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
  waitForTrace(timeoutMs = 20_000) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("Tracing.tracingComplete timed out")), timeoutMs);
      this.tracingComplete = () => { clearTimeout(timer); resolve(); };
    });
  }
}

const servers = { baseline: await serve(roots.baseline), final: await serve(roots.final) };
const profile = `/tmp/burrata-startup-paired-trace-${process.pid}`;
await fs.rm(profile, { recursive: true, force: true });
const chrome = spawn("chromium", [
  "--headless", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage",
  "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank",
], { stdio: "ignore" });
let socket;
try {
  let page;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const active = (await fs.readFile(path.join(profile, "DevToolsActivePort"), "utf8")).split("\n");
      const pages = await (await fetch(`http://127.0.0.1:${Number(active[0])}/json/list`)).json();
      page = pages.find(entry => entry.type === "page");
      if (page?.webSocketDebuggerUrl) break;
    } catch { /* Chromium is starting. */ }
    await delay(100);
  }
  if (!page?.webSocketDebuggerUrl) throw new Error("Chromium remote debugging endpoint did not start");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  const cdp = new CDP(socket);
  await cdp.call("Page.enable");
  await cdp.call("Runtime.enable");
  await cdp.call("Network.enable");
  await cdp.call("Tracing.start", {
    transferMode: "ReportEvents",
    categories: "devtools.timeline,blink.user_timing,disabled-by-default-devtools.timeline,disabled-by-default-v8.gc",
  });
  await cdp.call("Page.addScriptToEvaluateOnNewDocument", { source: `(() => {
    const m = window.__startupMeasure = { started: performance.now() };
    const key = new URL(location.href).searchParams.get('sample') || 'sample';
    const mark = name => performance.mark('burrata.' + key + '.' + name);
    const scan = () => {
      const shell = document.querySelector('.app-shell');
      if (shell && m.shellMs == null) m.shellMs = performance.now();
      const controls = document.querySelector('#burrata-playback-controls');
      const play = document.querySelector('button.play-button');
      if (controls && play && m.controlsMs == null) {
        m.controlsMs = performance.now();
        const interactionStarted = performance.now();
        mark('focus.begin');
        play.focus({ preventScroll: true });
        m.focusLatencyMs = performance.now() - interactionStarted;
        mark('focus.end');
        mark('click.begin');
        play.click();
        m.clickDispatchLatencyMs = performance.now() - interactionStarted - m.focusLatencyMs;
        mark('click.end');
        m.firstInteractionLatencyMs = performance.now() - interactionStarted;
        m.complete = true;
        observer.disconnect();
      }
    };
    const observer = new MutationObserver(scan);
    observer.observe(document, { childList: true, subtree: true });
    document.addEventListener('DOMContentLoaded', scan, { once: true });
    window.addEventListener('load', scan, { once: true });
  })();` });

  let navigation = 0;
  async function navigate(build, cache, label) {
    if (cache === "cold") await cdp.call("Network.clearBrowserCache");
    const url = `http://127.0.0.1:${servers[build].port}/?sample=${encodeURIComponent(label)}&nav=${++navigation}`;
    await cdp.call("Page.navigate", { url });
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline) {
      const state = await cdp.evaluate("window.__startupMeasure && JSON.stringify(window.__startupMeasure)");
      if (state) {
        const parsed = JSON.parse(state);
        if (parsed.complete) return { build, cache, label, navigation, ...parsed };
      }
      await delay(10);
    }
    const state = await cdp.evaluate("window.__startupMeasure && JSON.stringify(window.__startupMeasure)");
    throw new Error(`${label} did not reach core controls: ${state}`);
  }

  for (let warmup = 0; warmup < 3; warmup++) {
    const order = warmup % 2 === 0 ? ["baseline", "final"] : ["final", "baseline"];
    for (const build of order) await navigate(build, "warmup", `${build}.warmup.${warmup + 1}`);
  }
  const samples = [];
  const cycles = [];
  for (let cycle = 0; cycle < 20; cycle++) {
    const order = cycle % 2 === 0 ? ["baseline", "final"] : ["final", "baseline"];
    cycles.push({ cycle: cycle + 1, order: [...order] });
    for (const build of order) {
      const label = `${build}.cycle${String(cycle + 1).padStart(2, "0")}`;
      samples.push({ cycle: cycle + 1, order: order.join("-"), ...(await navigate(build, "cold", `${label}.cold`)) });
      samples.push({ cycle: cycle + 1, order: order.join("-"), ...(await navigate(build, "warm", `${label}.warm`)) });
    }
  }
  const traceDone = cdp.waitForTrace();
  await cdp.call("Tracing.end");
  await traceDone;

  const markName = event => {
    if (typeof event.name === "string" && event.name.startsWith("burrata.")) return event.name;
    const data = event.args?.data;
    for (const value of [data?.name, data?.message, data?.markName]) {
      if (typeof value === "string" && value.startsWith("burrata.")) return value;
    }
    return "";
  };
  const marks = new Map();
  for (const event of cdp.traceEvents) {
    const name = markName(event);
    if (name) marks.set(name, event.ts);
  }
  const attributed = samples.map(sample => {
    const start = marks.get(`burrata.${sample.label}.focus.begin`);
    const end = marks.get(`burrata.${sample.label}.focus.end`);
    const events = start == null || end == null ? [] : cdp.traceEvents.filter(event => {
      const ts = event.ts;
      const finish = ts == null ? ts : ts + (event.dur ?? 0);
      return ts != null && finish >= start && ts <= end && event.cat?.includes("devtools.timeline") && event.name !== "TimeStamp";
    });
    const counts = {};
    const durationsMs = {};
    for (const event of events) {
      counts[event.name] = (counts[event.name] ?? 0) + 1;
      durationsMs[event.name] = (durationsMs[event.name] ?? 0) + (event.dur ?? 0) / 1000;
    }
    const gc = events.filter(event => /GC/.test(event.name) || /\.gc/.test(event.cat ?? ""));
    return { ...sample, traceFocusWindowUs: start == null || end == null ? null : end - start, focusTraceEventCounts: counts,
      focusTraceDurationsMs: durationsMs, focusGcEvents: gc.map(event => ({ name: event.name, ts: event.ts, dur: event.dur ?? 0, cat: event.cat })) };
  });
  const result = {
    browser: "Chromium headless; one isolated profile for paired builds",
    baselineRoot: roots.baseline,
    finalRoot: roots.final,
    excludedWarmupsPerBuild: 3,
    measuredCycles: 20,
    measuredSamplesPerBuild: 40,
    sampling: "Cycles alternate baseline-first and final-first. Each build is measured cold after a browser cache clear, then warm immediately without an intervening cache clear.",
    traceCategories: "devtools.timeline, blink.user_timing, disabled-by-default-devtools.timeline, disabled-by-default-v8.gc",
    scope: "Diagnostic trace starts before navigation. Focus and synthetic click remain inline in the original MutationObserver callback; performance marks bracket focus and click. The trace is diagnostic and not native playback readiness or first audio.",
    cycles, samples: attributed, traceEventCount: cdp.traceEvents.length, traceEvents: cdp.traceEvents,
  };
  await fs.writeFile(outputPath, JSON.stringify(result, null, 2) + "\n");
  process.stdout.write(JSON.stringify({ outputPath, samples: attributed.length, traceEventCount: cdp.traceEvents.length }, null, 2) + "\n");
} finally {
  socket?.close();
  chrome.kill("SIGTERM");
  await Promise.all(Object.values(servers).map(({ server }) => new Promise(resolve => server.close(resolve))));
}
