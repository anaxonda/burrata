import assert from "node:assert/strict";
import http from "node:http";
import { test } from "node:test";
import { verifyFrontendModules } from "../verify-frontend-modules.mjs";

const fixtures = {
  "/src/main.ts": ["text/javascript; charset=utf-8", 'import { mount } from "/svelte.js"; import App from "/src/App.svelte"; import "/src/style.css"; mount(App, { target: document.getElementById("app") });'],
  "/src/App.svelte": ["text/javascript", 'import * as $ from "/node_modules/.vite/deps/svelte_internal_client.js?v=fixture"; export default function App($$anchor) { $.push({}, false); }'],
  "/src/style.css": ["text/css; charset=utf-8", ":root { color: #fff; }"],
};

async function serve(t, handler) {
  const requests = [];
  const server = http.createServer((request, response) => {
    requests.push({ path: request.url, accept: request.headers.accept });
    if (handler?.(request, response)) return;
    const fixture = fixtures[request.url];
    if (!fixture) { response.writeHead(404); response.end(); return; }
    response.writeHead(200, { "content-type": fixture[0] });
    response.end(fixture[1]);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  t.after(async () => {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  });
  return { origin: `http://127.0.0.1:${server.address().port}`, requests };
}

test("warms exactly the three fixed modules and requests direct CSS", async t => {
  const { origin, requests } = await serve(t);
  assert.deepEqual(await verifyFrontendModules({ origin }), Object.keys(fixtures));
  assert.deepEqual(requests.map(request => request.path), Object.keys(fixtures));
  assert.equal(requests[2].accept, "text/css");
});

for (const [name, path, type, body, expected] of [
  ["raw TypeScript entry", "/src/main.ts", "text/javascript", fixtures["/src/main.ts"][1].replace('getElementById("app")', 'getElementById("app")!'), /transformed frontend entry/],
  ["raw Svelte component with a misleading JS MIME", "/src/App.svelte", "text/javascript", '<script lang="ts">let title: string = "App";</script><main>{title}</main>', /compiled Svelte component/],
  ["HTML fallback", "/src/App.svelte", "text/html", "<!doctype html><html><body>fallback</body></html>", /JavaScript module/],
  ["HTML mislabeled as JavaScript", "/src/App.svelte", "application/javascript", "<html><body>fallback</body></html>", /JavaScript module/],
  ["style fallback HTML", "/src/style.css", "text/css", "<!doctype html><html>fallback</html>", /compiled CSS/],
  ["wrong style content type", "/src/style.css", "text/plain", ":root { color: red; }", /compiled CSS/],
]) {
  test(`rejects ${name}`, async t => {
    const { origin } = await serve(t, (request, response) => {
      if (request.url !== path) return false;
      response.writeHead(200, { "content-type": type }); response.end(body); return true;
    });
    await assert.rejects(verifyFrontendModules({ origin }), expected);
  });
}

test("rejects a missing stylesheet even when its body looks like CSS", async t => {
  const { origin } = await serve(t, (request, response) => {
    if (request.url !== "/src/style.css") return false;
    response.writeHead(404, { "content-type": "text/css" }); response.end(":root { color: red; }"); return true;
  });
  await assert.rejects(verifyFrontendModules({ origin }), /style\.css returned HTTP 404/);
});

for (const stalledBody of [false, true]) {
  test(`whole preflight deadline cancels stalled ${stalledBody ? "body" : "headers"}`, async t => {
    const { origin } = await serve(t, (request, response) => {
      if (request.url !== "/src/App.svelte") return false;
      if (stalledBody) { response.writeHead(200, { "content-type": "text/javascript" }); response.write("import "); }
      return true;
    });
    const started = Date.now();
    await assert.rejects(verifyFrontendModules({ origin, timeoutMs: 100 }), /exceeded 100 milliseconds|aborted/i);
    assert.ok(Date.now() - started < 1500, "deadline must include body reads and abort the pending fetch");
  });
}

test("caller cancellation aborts a pending component fetch", async t => {
  const controller = new AbortController();
  const { origin } = await serve(t, (request) => {
    if (request.url !== "/src/App.svelte") return false;
    controller.abort(new Error("fixture cancelled")); return true;
  });
  await assert.rejects(verifyFrontendModules({ origin, signal: controller.signal }), /fixture cancelled/);
});

test("rejects non-loopback origins before making requests", async () => {
  await assert.rejects(verifyFrontendModules({ origin: "https://example.com" }), /loopback HTTP origin/);
});
