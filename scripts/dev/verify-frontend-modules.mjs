import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";

const modules = [
  { path: "/src/main.ts", accept: "application/javascript", kind: "entry" },
  { path: "/src/App.svelte", accept: "application/javascript", kind: "component" },
  { path: "/src/style.css", accept: "text/css", kind: "style" },
];

function validateModule(module, response, body) {
  if (!response.ok) throw new Error(`${module.path} returned HTTP ${response.status}`);
  const type = (response.headers.get("content-type") ?? "").split(";", 1)[0].trim().toLowerCase();
  const html = /^\s*(?:<!doctype\s+html|<html\b|<head\b|<body\b)/i.test(body);
  if (module.kind === "style") {
    if (type !== "text/css" || html || /<\/?(?:script|style)\b/i.test(body)
        || !/[^{}\s][^{}]*\{[^{}]*\}/.test(body)) {
      throw new Error(`${module.path} is not a compiled CSS response (content-type: ${type || "missing"})`);
    }
    return;
  }
  if (!["text/javascript", "application/javascript"].includes(type) || html) {
    throw new Error(`${module.path} is not a JavaScript module (content-type: ${type || "missing"})`);
  }
  if (module.kind === "entry") {
    if (!/\bmount\b/.test(body) || !body.includes("App.svelte") || !body.includes("style.css")
        || /getElementById\s*\(\s*["']app["']\s*\)\s*!/.test(body)) {
      throw new Error(`${module.path} is not the transformed frontend entry`);
    }
  } else if (/^\s*<(?:script|svelte:|style)\b/i.test(body)
      || !/svelte(?:\/internal\/client|_internal_client)/.test(body)
      || !/\bexport\s+(?:default\b|\{[^}]*\bdefault\b)/.test(body)) {
    throw new Error(`${module.path} is not a compiled Svelte component`);
  }
}

// The CLI uses the fixed Vite origin. A loopback-only override allows offline
// HTTP fixtures to exercise the same fetch, body-read, timeout and validation.
export async function verifyFrontendModules({
  origin = "http://127.0.0.1:1420",
  timeoutMs = 60_000,
  signal,
} = {}) {
  const base = new URL(origin);
  if (base.protocol !== "http:" || !["127.0.0.1", "localhost", "[::1]"].includes(base.hostname)
      || base.username || base.password || base.pathname !== "/" || base.search || base.hash) {
    throw new Error("Frontend module verification requires a loopback HTTP origin");
  }
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 60_000) {
    throw new Error("Frontend preflight timeout must be between 1 and 60000 milliseconds");
  }
  const controller = new AbortController();
  const abort = () => controller.abort(signal.reason ?? new Error("Frontend preflight cancelled"));
  if (signal?.aborted) abort();
  else signal?.addEventListener("abort", abort, { once: true });
  const timer = setTimeout(() => controller.abort(new Error(`Frontend module preflight exceeded ${timeoutMs} milliseconds`)), timeoutMs);
  try {
    const verified = [];
    for (const module of modules) {
      let response;
      while (!response) {
        controller.signal.throwIfAborted();
        try {
          response = await fetch(new URL(module.path, base), {
            headers: { accept: module.accept },
            redirect: "error",
            signal: controller.signal,
          });
        } catch (error) {
          controller.signal.throwIfAborted();
          // Vite may not have bound its socket yet. Only initial connection
          // failures are retried; errors after any verified module fail closed.
          if (verified.length || error.cause?.code !== "ECONNREFUSED") throw error;
          await delay(250, undefined, { signal: controller.signal });
        }
      }
      const body = await response.text();
      validateModule(module, response, body);
      verified.push(module.path);
    }
    return verified;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", abort);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const controller = new AbortController();
  const interrupt = () => controller.abort(new Error("Frontend module preflight interrupted"));
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  try {
    const verified = await verifyFrontendModules({ signal: controller.signal });
    console.log(`Verified compiled frontend modules: ${verified.join(", ")}.`);
  } catch (error) {
    console.error(`Frontend module verification failed: ${error.message}`);
    process.exitCode = controller.signal.aborted ? 130 : 1;
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
  }
}
