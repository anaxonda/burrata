// Dedicated process: upstream BgUtils owns globals and has non-cancellable retries.
// The parent can kill this entire process on the command's absolute deadline.
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';
const emit = value => { const used = process.cpuUsage(); process.stdout.write(JSON.stringify({ ...value, tokenProcessCpuMs: (used.user + used.system) / 1000, tokenProcessMaxRssBytes: process.resourceUsage().maxRSS * 1024 }) + '\n'); };
// Upstream warnings can contain bindings, URLs and tokens; discard every log.
for (const name of ['log', 'info', 'warn', 'error', 'debug']) console[name] = () => {};
let manager;
for await (const line of createInterface({ input: process.stdin })) {
  try {
    const { context, bypassCache = false } = JSON.parse(line);
    const binding = context?.client?.visitorData;
    if (typeof binding !== 'string' || !binding) throw new Error();
    if (!manager) {
      const { SessionManager } = await import(pathToFileURL(process.env.BURRATA_BENCH_BGUTIL_MODULE));
      manager = new SessionManager(false, {});
      manager.logger = { log() {}, debug() {}, warn() {}, error() {} };
    }
    const result = await manager.generatePoToken(binding, '', bypassCache, undefined, false, undefined, context);
    const expiresAt = new Date(result.expiresAt).getTime();
    if (!result.poToken || !Number.isFinite(expiresAt) || expiresAt <= Date.now() + 30_000) throw new Error();
    emit({ ok: true, poToken: result.poToken, expiresAt, contentBinding: binding });
  } catch { emit({ ok: false }); }
}
