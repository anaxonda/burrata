import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { access } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { failure } from './budget.mjs';

export function createTokenManager({ modulePath = fileURLToPath(new URL('../../../src-tauri/resources/youtube-support/bgutil/server/build/session_manager.js', import.meta.url)), spawnChild = spawn, now = Date.now } = {}) {
  let child, reader, pending, cached, lastCpuMs = 0;
  const stop = () => {
    cached = null; lastCpuMs = 0;
    if (child) {
      child.kill('SIGTERM');
      const pid = child.pid;
      const timer = setTimeout(() => { try { process.kill(-pid, 'SIGKILL'); } catch {} }, 250);
      timer.unref();
      child.once('close', () => clearTimeout(timer));
    }
    child = null;
    reader?.close();
    pending?.reject(failure('TOKEN_PROVIDER_FAILED', 'token'));
    pending = null;
  };
  return {
    stop,
    async ensure(client, { signal, refresh = false } = {}) {
      const binding = client.session.context?.client?.visitorData;
      if (!binding) throw failure('TOKEN_BINDING_MISSING', 'token');
      let cacheHit = cached?.binding === binding && cached.expiresAt > now() + 30_000;
      if (!cacheHit || refresh) {
        if (!child) {
          try { await access(modulePath); } catch { throw failure('TOKEN_PROVIDER_UNAVAILABLE', 'token'); }
          child = spawnChild(process.execPath, [fileURLToPath(new URL('./token_child.mjs', import.meta.url))], {
            stdio: ['pipe', 'pipe', 'ignore'], detached: process.platform !== 'win32',
            env: { ...process.env, BURRATA_BENCH_BGUTIL_MODULE: modulePath },
          });
          reader = createInterface({ input: child.stdout });
          reader.on('line', line => {
            const request = pending; pending = null;
            try { request?.resolve(JSON.parse(line)); } catch { request?.reject(failure('TOKEN_PROVIDER_FAILED', 'token')); }
          });
          const startedChild = child;
          child.once('error', () => { if (child === startedChild) stop(); });
          child.once('exit', () => { if (child === startedChild) stop(); });
        }
        signal?.throwIfAborted();
        const abort = () => stop();
        signal?.addEventListener('abort', abort, { once: true });
        try {
          const result = await new Promise((resolve, reject) => {
            pending = { resolve, reject };
            child.stdin.write(JSON.stringify({ context: client.session.context, bypassCache: refresh }) + '\n');
          });
          if (!result.ok || result.contentBinding !== binding || !Number.isFinite(result.expiresAt) || result.expiresAt <= now() + 30_000) throw failure('TOKEN_PROVIDER_FAILED', 'token');
          cached = { binding, token: result.poToken, expiresAt: result.expiresAt, cpuMs: result.tokenProcessCpuMs - lastCpuMs, maxRssBytes: result.tokenProcessMaxRssBytes };
          lastCpuMs = result.tokenProcessCpuMs;
          cacheHit = false;
        } finally { signal?.removeEventListener('abort', abort); }
      }
      client.session.po_token = cached.token;
      if (client.session.player) client.session.player.po_token = cached.token;
      return { tokenCacheHit: Boolean(cacheHit), tokenExpiresInMs: cached.expiresAt - now(), tokenHelperCpuMs: cacheHit ? 0 : cached.cpuMs, tokenHelperMaxRssBytes: cached.maxRssBytes };
    },
  };
}
